//! RSA Key Management for K2K Authentication

use anyhow::{Context, Result};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use rsa::{
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, LineEnding},
    RsaPrivateKey, RsaPublicKey,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tracing::{error, info};

use super::models::{ClientKey, K2KClaims};
use crate::store::Store;

pub struct KeyManager {
    #[allow(dead_code)]
    private_key: RsaPrivateKey,
    public_key: RsaPublicKey,
    registered_clients: HashMap<String, ClientKey>,
    config_dir: PathBuf,
    db: Option<Arc<dyn Store>>,
}

impl KeyManager {
    /// Create or load KeyManager with optional database backing
    pub async fn new(config_dir: impl AsRef<Path>, db: Option<Arc<dyn Store>>) -> Result<Self> {
        let config_dir = config_dir.as_ref().to_path_buf();

        // Ensure config directory exists
        fs::create_dir_all(&config_dir)
            .await
            .context("Failed to create config directory")?;

        let private_key_path = config_dir.join("k2k_private_key.pem");
        let public_key_path = config_dir.join("k2k_public_key.pem");
        let clients_path = config_dir.join("k2k_clients.json");

        // Load or generate RSA keys
        let (private_key, public_key) = if private_key_path.exists() {
            info!("Loading existing RSA key pair for K2K authentication");
            Self::load_keys(&private_key_path, &public_key_path).await?
        } else {
            info!("Generating new RSA key pair for K2K authentication (2048-bit)");
            let private_key = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048)
                .context("Failed to generate RSA private key")?;
            let public_key = RsaPublicKey::from(&private_key);

            Self::save_keys(
                &private_key,
                &public_key,
                &private_key_path,
                &public_key_path,
            )
            .await?;

            (private_key, public_key)
        };

        // Load registered clients from DB first, then fall back to JSON
        let registered_clients = if let Some(ref db) = db {
            let db_clients = db.list_k2k_clients().await?;
            if db_clients.is_empty() && clients_path.exists() {
                // Migrate from JSON to DB
                info!("Migrating K2K clients from JSON to SQLite");
                let json_clients = Self::load_clients_from_json(&clients_path).await?;
                for client in json_clients.values() {
                    let db_client = crate::store::K2KClient {
                        client_id: client.client_id.clone(),
                        public_key_pem: client.public_key_pem.clone(),
                        client_name: client.client_name.clone(),
                        registered_at: client.registered_at.to_rfc3339(),
                        status: "approved".to_string(), // Migrated clients are auto-approved
                    };
                    db.upsert_k2k_client(&db_client).await?;
                }
                json_clients
            } else {
                db_clients
                    .into_iter()
                    .map(|c| {
                        let key = ClientKey {
                            client_id: c.client_id.clone(),
                            client_name: c.client_name,
                            public_key_pem: c.public_key_pem,
                            registered_at: chrono::DateTime::parse_from_rfc3339(&c.registered_at)
                                .map(|dt| dt.with_timezone(&chrono::Utc))
                                .unwrap_or_else(|_| chrono::Utc::now()),
                        };
                        (c.client_id, key)
                    })
                    .collect()
            }
        } else if clients_path.exists() {
            Self::load_clients_from_json(&clients_path).await?
        } else {
            HashMap::new()
        };

        info!(
            "K2K KeyManager initialized with {} registered clients",
            registered_clients.len()
        );

        Ok(Self {
            private_key,
            public_key,
            registered_clients,
            config_dir,
            db,
        })
    }

    /// Get public key in PEM format
    pub fn get_public_key_pem(&self) -> String {
        self.public_key
            .to_public_key_pem(LineEnding::LF)
            .unwrap_or_else(|e| {
                error!("Failed to encode public key to PEM: {}", e);
                String::new()
            })
    }

    /// Register a client's public key with a given approval status
    pub async fn register_client(
        &mut self,
        client_id: String,
        client_name: String,
        public_key_pem: String,
        status: &str,
    ) -> Result<()> {
        // Validate PEM format by trying to parse it (accept both PKCS#8 and PKCS#1)
        let _public_key = RsaPublicKey::from_public_key_pem(&public_key_pem)
            .or_else(|_| {
                use rsa::pkcs1::DecodeRsaPublicKey;
                RsaPublicKey::from_pkcs1_pem(&public_key_pem)
            })
            .context("Invalid public key PEM format")?;

        let client_key = ClientKey {
            client_id: client_id.clone(),
            client_name: client_name.clone(),
            public_key_pem: public_key_pem.clone(),
            registered_at: chrono::Utc::now(),
        };

        self.registered_clients
            .insert(client_id.clone(), client_key.clone());

        // Persist to DB if available, otherwise fall back to JSON
        if let Some(ref db) = self.db {
            let db_client = crate::store::K2KClient {
                client_id: client_key.client_id,
                public_key_pem: client_key.public_key_pem,
                client_name: client_key.client_name,
                registered_at: client_key.registered_at.to_rfc3339(),
                status: status.to_string(),
            };
            db.upsert_k2k_client(&db_client).await?;
        } else {
            self.save_clients_to_json().await?;
        }

        info!("Registered K2K client: {}", client_id);

        Ok(())
    }

    /// Verify JWT signature and extract claims
    pub fn verify_jwt(&self, jwt: &str, expected_client_id: &str) -> Result<K2KClaims> {
        // First, decode without verification to get client_id and get the right public key
        let parts: Vec<&str> = jwt.split('.').collect();
        if parts.len() != 3 {
            anyhow::bail!("Invalid JWT format: expected 3 parts, got {}", parts.len());
        }

        let payload_b64 = parts[1];
        let payload_bytes = Self::base64url_decode(payload_b64)?;
        let claims: K2KClaims =
            serde_json::from_slice(&payload_bytes).context("Failed to parse JWT claims")?;

        // Validate issuer
        let expected_iss = format!("kb:{}", expected_client_id);
        if claims.iss != expected_iss {
            anyhow::bail!(
                "Invalid issuer: expected '{}', got '{}'",
                expected_iss,
                claims.iss
            );
        }

        // Get client's public key
        let client = self
            .registered_clients
            .get(&claims.client_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown client: {}", claims.client_id))?;

        // Use jsonwebtoken crate for proper RS256 verification
        let decoding_key = DecodingKey::from_rsa_pem(client.public_key_pem.as_bytes())
            .context("Failed to create decoding key from public key")?;

        let mut validation = Validation::new(Algorithm::RS256);
        // Disable default validations - we'll do them ourselves for better error messages
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.set_audience(&[] as &[&str]); // Disable audience validation
        validation.set_required_spec_claims(&["iss", "exp", "iat"]);

        // Verify signature using jsonwebtoken
        let _token_data = decode::<K2KClaims>(jwt, &decoding_key, &validation)
            .context("Signature verification failed")?;

        // Now validate expiration manually for better error message
        let now = chrono::Utc::now().timestamp();
        if claims.exp < now {
            anyhow::bail!("JWT expired (exp: {}, now: {})", claims.exp, now);
        }

        // Validate issued-at (not in the future)
        if claims.iat > now + 60 {
            anyhow::bail!(
                "JWT issued in the future (iat: {}, now: {})",
                claims.iat,
                now
            );
        }

        tracing::debug!(
            "JWT verified successfully for client: {}",
            expected_client_id
        );

        Ok(claims)
    }

    /// Get list of registered clients
    #[allow(dead_code)]
    pub fn list_clients(&self) -> Vec<&ClientKey> {
        self.registered_clients.values().collect()
    }

    /// Check if a client is registered
    #[allow(dead_code)]
    pub fn is_client_registered(&self, client_id: &str) -> bool {
        self.registered_clients.contains_key(client_id)
    }

    /// Get a registered client by ID
    #[allow(dead_code)]
    pub fn get_client(&self, client_id: &str) -> Option<&ClientKey> {
        self.registered_clients.get(client_id)
    }

    /// Get client status from DB (approved/pending/rejected)
    pub async fn get_client_status(&self, client_id: &str) -> Option<String> {
        if let Some(ref db) = self.db {
            db.get_k2k_client(client_id)
                .await
                .ok()
                .flatten()
                .map(|c| c.status)
        } else {
            // Without DB, all registered clients are considered approved
            if self.registered_clients.contains_key(client_id) {
                Some("approved".to_string())
            } else {
                None
            }
        }
    }

    // ========================================================================
    // Private Helper Methods
    // ========================================================================

    async fn load_keys(
        private_key_path: &Path,
        public_key_path: &Path,
    ) -> Result<(RsaPrivateKey, RsaPublicKey)> {
        let private_pem = fs::read_to_string(private_key_path)
            .await
            .context("Failed to read private key file")?;
        let public_pem = fs::read_to_string(public_key_path)
            .await
            .context("Failed to read public key file")?;

        let private_key = RsaPrivateKey::from_pkcs8_pem(&private_pem)
            .context("Failed to parse private key PEM")?;
        let public_key = RsaPublicKey::from_public_key_pem(&public_pem)
            .context("Failed to parse public key PEM")?;

        Ok((private_key, public_key))
    }

    async fn save_keys(
        private_key: &RsaPrivateKey,
        public_key: &RsaPublicKey,
        private_key_path: &Path,
        public_key_path: &Path,
    ) -> Result<()> {
        let private_pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .context("Failed to encode private key to PEM")?;
        let public_pem = public_key
            .to_public_key_pem(LineEnding::LF)
            .context("Failed to encode public key to PEM")?;

        fs::write(private_key_path, private_pem.as_bytes())
            .await
            .context("Failed to write private key file")?;
        // Restrict private key to owner read/write only (0o600) so other
        // local users cannot read the key and forge K2K JWT tokens.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                private_key_path,
                std::fs::Permissions::from_mode(0o600),
            )
            .context("Failed to set private key file permissions")?;
        }
        fs::write(public_key_path, public_pem.as_bytes())
            .await
            .context("Failed to write public key file")?;

        info!(
            "Saved RSA key pair to {:?}",
            private_key_path.parent().unwrap_or(Path::new("."))
        );

        Ok(())
    }

    async fn load_clients_from_json(clients_path: &Path) -> Result<HashMap<String, ClientKey>> {
        let contents = fs::read_to_string(clients_path)
            .await
            .context("Failed to read clients file")?;
        let clients: Vec<ClientKey> =
            serde_json::from_str(&contents).context("Failed to parse clients JSON")?;

        Ok(clients
            .into_iter()
            .map(|c| (c.client_id.clone(), c))
            .collect())
    }

    async fn save_clients_to_json(&self) -> Result<()> {
        let clients_path = self.config_dir.join("k2k_clients.json");
        let clients: Vec<&ClientKey> = self.registered_clients.values().collect();
        let json = serde_json::to_string_pretty(&clients).context("Failed to serialize clients")?;

        fs::write(&clients_path, json)
            .await
            .context("Failed to write clients file")?;

        Ok(())
    }

    fn base64url_decode(input: &str) -> Result<Vec<u8>> {
        use base64::Engine;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        engine.decode(input).context("Failed to decode base64url")
    }
}
