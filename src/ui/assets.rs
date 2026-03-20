//! Embedded web assets for the local UI.

/// Main HTML page
pub const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Knowledge Nexus - Local Search</title>
    <style>
        * { margin: 0; padding: 0; box-sizing: border-box; }

        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
            background: linear-gradient(135deg, #1a1a2e 0%, #16213e 100%);
            color: #e0e0e0;
            min-height: 100vh;
            padding: 2rem;
        }

        .container { max-width: 900px; margin: 0 auto; }

        header { text-align: center; margin-bottom: 2rem; }
        header h1 { font-size: 2rem; color: #00d4ff; margin-bottom: 0.5rem; }
        header p { color: #888; font-size: 0.9rem; }

        /* Tab navigation */
        .tabs {
            display: flex;
            gap: 0;
            margin-bottom: 1.5rem;
            border-bottom: 2px solid rgba(255,255,255,0.1);
        }
        .tab {
            padding: 0.75rem 1.5rem;
            background: none;
            border: none;
            color: #888;
            font-size: 0.95rem;
            cursor: pointer;
            border-bottom: 2px solid transparent;
            margin-bottom: -2px;
            transition: color 0.2s, border-color 0.2s;
        }
        .tab:hover { color: #ccc; }
        .tab.active {
            color: #00d4ff;
            border-bottom-color: #00d4ff;
        }
        .tab-content { display: none; }
        .tab-content.active { display: block; }

        /* Search tab */
        .search-box {
            background: rgba(255,255,255,0.05);
            border-radius: 12px;
            padding: 1.5rem;
            margin-bottom: 1.5rem;
        }
        .search-input-container { display: flex; gap: 1rem; }
        #searchInput {
            flex: 1;
            padding: 1rem 1.5rem;
            border: 2px solid rgba(0,212,255,0.3);
            border-radius: 8px;
            background: rgba(0,0,0,0.3);
            color: #fff;
            font-size: 1rem;
            transition: border-color 0.3s;
        }
        #searchInput:focus { outline: none; border-color: #00d4ff; }
        #searchInput::placeholder { color: #666; }

        .btn-primary {
            padding: 0.75rem 1.5rem;
            background: linear-gradient(135deg, #00d4ff 0%, #0094ff 100%);
            border: none;
            border-radius: 8px;
            color: #fff;
            font-size: 0.95rem;
            font-weight: 600;
            cursor: pointer;
            transition: transform 0.2s, box-shadow 0.2s;
        }
        .btn-primary:hover {
            transform: translateY(-1px);
            box-shadow: 0 4px 20px rgba(0,212,255,0.4);
        }
        .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; transform: none; }

        .btn-danger {
            padding: 0.4rem 0.8rem;
            background: rgba(244,67,54,0.2);
            border: 1px solid rgba(244,67,54,0.4);
            border-radius: 6px;
            color: #f44336;
            font-size: 0.8rem;
            cursor: pointer;
            transition: background 0.2s;
        }
        .btn-danger:hover { background: rgba(244,67,54,0.35); }

        .btn-secondary {
            padding: 0.5rem 1rem;
            background: rgba(255,255,255,0.1);
            border: 1px solid rgba(255,255,255,0.2);
            border-radius: 6px;
            color: #ccc;
            font-size: 0.85rem;
            cursor: pointer;
            transition: background 0.2s;
        }
        .btn-secondary:hover { background: rgba(255,255,255,0.18); }

        .status-bar {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 0.75rem 1rem;
            background: rgba(0,0,0,0.2);
            border-radius: 8px;
            margin-bottom: 1.5rem;
            font-size: 0.85rem;
        }
        .status-indicator { display: flex; align-items: center; gap: 0.5rem; }
        .status-dot { width: 8px; height: 8px; border-radius: 50%; background: #4caf50; }
        .status-dot.offline { background: #ff9800; }

        #results { display: flex; flex-direction: column; gap: 1rem; }

        .result-card {
            background: rgba(255,255,255,0.05);
            border-radius: 10px;
            padding: 1.25rem;
            border-left: 3px solid #00d4ff;
            transition: transform 0.2s, background 0.2s;
        }
        .result-card:hover { background: rgba(255,255,255,0.08); transform: translateX(5px); }
        .result-header { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 0.75rem; }
        .result-filename { font-size: 1.1rem; font-weight: 600; color: #00d4ff; }
        .result-score {
            background: rgba(0,212,255,0.2); padding: 0.25rem 0.75rem;
            border-radius: 12px; font-size: 0.8rem; color: #00d4ff;
        }
        .result-path { font-size: 0.85rem; color: #888; margin-bottom: 0.75rem; word-break: break-all; }
        .result-snippet {
            background: rgba(0,0,0,0.3); padding: 0.75rem 1rem; border-radius: 6px;
            font-family: 'Monaco','Menlo',monospace; font-size: 0.85rem; color: #ccc;
            white-space: pre-wrap; overflow-x: auto;
        }
        .result-meta { display: flex; gap: 1.5rem; margin-top: 0.75rem; font-size: 0.8rem; color: #666; }
        .no-results { text-align: center; padding: 3rem; color: #666; }
        .loading { text-align: center; padding: 2rem; color: #00d4ff; }
        .loading::after { content: ''; animation: dots 1.5s infinite; }
        @keyframes dots { 0%,20%{content:'.'} 40%{content:'..'} 60%,100%{content:'...'} }
        .error { background: rgba(244,67,54,0.1); border-left: 3px solid #f44336; padding: 1rem; border-radius: 8px; color: #f44336; }

        /* Settings tab */
        .settings-section {
            background: rgba(255,255,255,0.05);
            border-radius: 12px;
            padding: 1.5rem;
            margin-bottom: 1.5rem;
        }
        .settings-section h3 {
            color: #00d4ff;
            margin-bottom: 1rem;
            font-size: 1.1rem;
        }

        .path-list { list-style: none; }
        .path-item {
            display: flex;
            justify-content: space-between;
            align-items: center;
            padding: 0.6rem 0.8rem;
            border-radius: 6px;
            margin-bottom: 0.5rem;
            background: rgba(0,0,0,0.2);
        }
        .path-item .path-text { font-family: monospace; font-size: 0.9rem; word-break: break-all; flex: 1; }
        .path-item .path-missing { color: #ff9800; font-size: 0.8rem; margin-left: 0.5rem; }
        .path-item .path-ok { color: #4caf50; font-size: 0.8rem; margin-left: 0.5rem; }

        .add-path-row { display: flex; gap: 0.75rem; margin-top: 1rem; }
        .add-path-row input {
            flex: 1;
            padding: 0.6rem 1rem;
            border: 1px solid rgba(255,255,255,0.15);
            border-radius: 6px;
            background: rgba(0,0,0,0.3);
            color: #fff;
            font-family: monospace;
            font-size: 0.9rem;
        }
        .add-path-row input:focus { outline: none; border-color: #00d4ff; }
        .add-path-row input::placeholder { color: #555; }

        .hub-form { display: flex; flex-direction: column; gap: 0.75rem; }
        .hub-form label { color: #aaa; font-size: 0.85rem; }
        .hub-form input {
            padding: 0.6rem 1rem;
            border: 1px solid rgba(255,255,255,0.15);
            border-radius: 6px;
            background: rgba(0,0,0,0.3);
            color: #fff;
            font-size: 0.9rem;
        }
        .hub-form input:focus { outline: none; border-color: #00d4ff; }
        .hub-form input::placeholder { color: #555; }
        .hub-form .btn-row { display: flex; gap: 0.75rem; margin-top: 0.5rem; }

        .toast {
            position: fixed;
            bottom: 2rem;
            right: 2rem;
            padding: 0.75rem 1.5rem;
            border-radius: 8px;
            color: #fff;
            font-size: 0.9rem;
            opacity: 0;
            transition: opacity 0.3s;
            z-index: 1000;
            max-width: 400px;
        }
        .toast.show { opacity: 1; }
        .toast.success { background: rgba(76,175,80,0.9); }
        .toast.error { background: rgba(244,67,54,0.9); }

        .device-info {
            display: grid;
            grid-template-columns: auto 1fr;
            gap: 0.4rem 1rem;
            font-size: 0.9rem;
        }
        .device-info dt { color: #888; }
        .device-info dd { font-family: monospace; }

        footer {
            text-align: center; margin-top: 3rem; padding-top: 1.5rem;
            border-top: 1px solid rgba(255,255,255,0.1); color: #666; font-size: 0.85rem;
        }
        footer a { color: #00d4ff; text-decoration: none; }
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>Knowledge Nexus</h1>
            <p>Local Semantic Search</p>
        </header>

        <div class="tabs">
            <button class="tab active" data-tab="search">Search</button>
            <button class="tab" data-tab="settings">Settings</button>
        </div>

        <!-- Search Tab -->
        <div class="tab-content active" id="tab-search">
            <div class="search-box">
                <div class="search-input-container">
                    <input type="text" id="searchInput" placeholder="Search your files semantically..." autofocus>
                    <button class="btn-primary" id="searchButton">Search</button>
                </div>
            </div>

            <div class="status-bar">
                <div class="status-indicator">
                    <div class="status-dot" id="statusDot"></div>
                    <span id="statusText">Checking status...</span>
                </div>
                <div id="indexInfo">-</div>
            </div>

            <div id="results"></div>
        </div>

        <!-- Settings Tab -->
        <div class="tab-content" id="tab-settings">
            <!-- Device info -->
            <div class="settings-section">
                <h3>Device</h3>
                <dl class="device-info" id="deviceInfo">
                    <dt>Name</dt><dd id="deviceName">-</dd>
                    <dt>ID</dt><dd id="deviceId">-</dd>
                </dl>
            </div>

            <!-- Indexed Paths -->
            <div class="settings-section">
                <h3>Indexed Paths</h3>
                <p style="color:#888; font-size:0.85rem; margin-bottom:0.75rem;">
                    Directories that the agent indexes for semantic search.
                </p>
                <ul class="path-list" id="pathList"></ul>
                <div class="add-path-row">
                    <input type="text" id="newPathInput" placeholder="C:\Users\you\Documents or ~/Projects">
                    <button class="btn-primary" id="addPathBtn">Add Path</button>
                </div>
                <div style="margin-top:1rem;">
                    <button class="btn-secondary" id="reindexBtn">Reindex Files</button>
                    <span id="reindexStatus" style="margin-left:0.75rem; font-size:0.85rem; color:#888;"></span>
                </div>
            </div>

            <!-- Hub Connection -->
            <div class="settings-section">
                <h3>Knowledge Nexus Hub</h3>
                <p style="color:#888; font-size:0.85rem; margin-bottom:0.75rem;">
                    Connect this agent to a Knowledge Nexus Hub to enable federated search across devices.
                </p>
                <div class="hub-form">
                    <label>Hub URL</label>
                    <input type="text" id="hubUrl" placeholder="wss://hub.example.com/ws">
                    <label>Auth Token (optional)</label>
                    <input type="password" id="hubToken" placeholder="JWT token or leave empty">
                    <div class="btn-row">
                        <button class="btn-primary" id="saveHubBtn">Save</button>
                        <button class="btn-secondary" id="clearHubBtn">Disconnect</button>
                    </div>
                </div>
            </div>
        </div>

        <footer>
            <p>Knowledge Nexus System Agent v0.8.0</p>
            <p><a href="https://github.com/jaredcluff/knowledge-nexus-local">Documentation</a></p>
        </footer>
    </div>

    <!-- Toast notification -->
    <div class="toast" id="toast"></div>

    <script>
        // UI token injected at server startup — included in mutation requests so
        // that other local processes cannot reconfigure the agent without it.
        const UI_TOKEN = '__UI_TOKEN_PLACEHOLDER__';
        const MUTATION_HEADERS = { 'Content-Type': 'application/json', 'X-UI-Token': UI_TOKEN };

        // Elements
        const searchInput = document.getElementById('searchInput');
        const searchButton = document.getElementById('searchButton');
        const resultsDiv = document.getElementById('results');
        const statusDot = document.getElementById('statusDot');
        const statusText = document.getElementById('statusText');
        const indexInfo = document.getElementById('indexInfo');
        const pathList = document.getElementById('pathList');
        const newPathInput = document.getElementById('newPathInput');
        const addPathBtn = document.getElementById('addPathBtn');
        const reindexBtn = document.getElementById('reindexBtn');
        const reindexStatus = document.getElementById('reindexStatus');
        const hubUrlInput = document.getElementById('hubUrl');
        const hubTokenInput = document.getElementById('hubToken');
        const saveHubBtn = document.getElementById('saveHubBtn');
        const clearHubBtn = document.getElementById('clearHubBtn');

        // Tab switching
        document.querySelectorAll('.tab').forEach(tab => {
            tab.addEventListener('click', () => {
                document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
                document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
                tab.classList.add('active');
                document.getElementById('tab-' + tab.dataset.tab).classList.add('active');
                if (tab.dataset.tab === 'settings') loadSettings();
            });
        });

        // Toast
        function showToast(message, type) {
            const toast = document.getElementById('toast');
            toast.textContent = message;
            toast.className = 'toast show ' + type;
            setTimeout(() => toast.classList.remove('show'), 3500);
        }

        // ---- Status ----
        async function checkStatus() {
            try {
                const res = await fetch('/api/status');
                const data = await res.json();
                statusText.textContent = data.hub_connected ? 'Connected to Hub' : 'Offline Mode';
                statusDot.className = data.hub_connected ? 'status-dot' : 'status-dot offline';
                indexInfo.textContent = data.indexed_paths.length + ' path' + (data.indexed_paths.length !== 1 ? 's' : '') + ' configured';
            } catch (e) {
                statusText.textContent = 'Error checking status';
                statusDot.className = 'status-dot offline';
            }
        }

        // ---- Search ----
        async function search() {
            const query = searchInput.value.trim();
            if (!query) return;
            searchButton.disabled = true;
            resultsDiv.innerHTML = '<div class="loading">Searching</div>';
            try {
                const res = await fetch('/api/search?q=' + encodeURIComponent(query) + '&limit=20');
                const data = await res.json();
                if (!data.success) {
                    resultsDiv.innerHTML = '<div class="error">Error: ' + escapeHtml(data.error) + '</div>';
                    return;
                }
                if (data.results.length === 0) {
                    resultsDiv.innerHTML = '<div class="no-results">No results found. Try a different query.</div>';
                    return;
                }
                resultsDiv.innerHTML = data.results.map(r =>
                    '<div class="result-card">' +
                        '<div class="result-header">' +
                            '<span class="result-filename">' + escapeHtml(r.filename) + '</span>' +
                            '<span class="result-score">' + (r.score * 100).toFixed(1) + '%</span>' +
                        '</div>' +
                        '<div class="result-path">' + escapeHtml(r.path) + '</div>' +
                        (r.snippet ? '<div class="result-snippet">' + escapeHtml(r.snippet) + '</div>' : '') +
                        '<div class="result-meta">' +
                            (r.size_bytes ? '<span>Size: ' + formatSize(r.size_bytes) + '</span>' : '') +
                            (r.modified_at ? '<span>Modified: ' + formatDate(r.modified_at) + '</span>' : '') +
                        '</div>' +
                    '</div>'
                ).join('');
            } catch (e) {
                resultsDiv.innerHTML = '<div class="error">Search failed: ' + escapeHtml(e.message) + '</div>';
            } finally {
                searchButton.disabled = false;
            }
        }

        searchButton.addEventListener('click', search);
        searchInput.addEventListener('keypress', e => { if (e.key === 'Enter') search(); });

        // ---- Settings ----
        async function loadSettings() {
            try {
                const res = await fetch('/api/status');
                const data = await res.json();

                // Device info
                document.getElementById('deviceName').textContent = data.device_name;
                document.getElementById('deviceId').textContent = data.device_id;

                // Paths
                pathList.innerHTML = '';
                if (data.indexed_paths.length === 0) {
                    pathList.innerHTML = '<li class="path-item"><span class="path-text" style="color:#666;">(no paths configured)</span></li>';
                } else {
                    data.indexed_paths.forEach(p => {
                        const li = document.createElement('li');
                        li.className = 'path-item';
                        const pathSpan = document.createElement('span');
                        pathSpan.className = 'path-text';
                        pathSpan.textContent = p.path;
                        li.appendChild(pathSpan);
                        const statusSpan = document.createElement('span');
                        statusSpan.className = p.exists ? 'path-ok' : 'path-missing';
                        statusSpan.textContent = p.exists ? 'OK' : 'missing';
                        li.appendChild(statusSpan);
                        const removeBtn = document.createElement('button');
                        removeBtn.className = 'btn-danger';
                        removeBtn.textContent = 'Remove';
                        removeBtn.addEventListener('click', () => removePath(p.path));
                        li.appendChild(removeBtn);
                        pathList.appendChild(li);
                    });
                }

                // Hub
                hubUrlInput.value = data.hub_url || '';
            } catch (e) {
                showToast('Failed to load settings', 'error');
            }
        }

        // Add path
        addPathBtn.addEventListener('click', async () => {
            const path = newPathInput.value.trim();
            if (!path) return;
            addPathBtn.disabled = true;
            try {
                const res = await fetch('/api/paths', {
                    method: 'POST',
                    headers: MUTATION_HEADERS,
                    body: JSON.stringify({ path })
                });
                const data = await res.json();
                if (data.success) {
                    showToast(data.message, 'success');
                    newPathInput.value = '';
                    loadSettings();
                    checkStatus();
                } else {
                    showToast(data.message, 'error');
                }
            } catch (e) {
                showToast('Failed to add path: ' + e.message, 'error');
            } finally {
                addPathBtn.disabled = false;
            }
        });

        newPathInput.addEventListener('keypress', e => {
            if (e.key === 'Enter') addPathBtn.click();
        });

        // Remove path
        async function removePath(path) {
            try {
                const res = await fetch('/api/paths', {
                    method: 'DELETE',
                    headers: MUTATION_HEADERS,
                    body: JSON.stringify({ path })
                });
                const data = await res.json();
                if (data.success) {
                    showToast(data.message, 'success');
                    loadSettings();
                    checkStatus();
                } else {
                    showToast(data.message, 'error');
                }
            } catch (e) {
                showToast('Failed to remove path: ' + e.message, 'error');
            }
        }

        // Reindex
        reindexBtn.addEventListener('click', async () => {
            reindexBtn.disabled = true;
            reindexStatus.textContent = 'Starting reindex...';
            try {
                const res = await fetch('/api/reindex', { method: 'POST', headers: MUTATION_HEADERS });
                const data = await res.json();
                if (data.success) {
                    reindexStatus.textContent = 'Reindex running in background.';
                    showToast('Reindex started', 'success');
                } else {
                    reindexStatus.textContent = 'Failed: ' + data.message;
                    showToast(data.message, 'error');
                }
            } catch (e) {
                reindexStatus.textContent = 'Error: ' + e.message;
                showToast('Reindex failed', 'error');
            } finally {
                setTimeout(() => { reindexBtn.disabled = false; }, 5000);
            }
        });

        // Hub save
        saveHubBtn.addEventListener('click', async () => {
            saveHubBtn.disabled = true;
            try {
                const res = await fetch('/api/hub', {
                    method: 'PUT',
                    headers: MUTATION_HEADERS,
                    body: JSON.stringify({
                        hub_url: hubUrlInput.value.trim() || null,
                        auth_token: hubTokenInput.value.trim() || null
                    })
                });
                const data = await res.json();
                if (data.success) {
                    showToast(data.message, 'success');
                    hubTokenInput.value = '';
                    checkStatus();
                } else {
                    showToast(data.message, 'error');
                }
            } catch (e) {
                showToast('Failed to save hub config', 'error');
            } finally {
                saveHubBtn.disabled = false;
            }
        });

        // Hub disconnect
        clearHubBtn.addEventListener('click', async () => {
            clearHubBtn.disabled = true;
            try {
                const res = await fetch('/api/hub', {
                    method: 'PUT',
                    headers: MUTATION_HEADERS,
                    body: JSON.stringify({ hub_url: null, auth_token: null })
                });
                const data = await res.json();
                if (data.success) {
                    hubUrlInput.value = '';
                    hubTokenInput.value = '';
                    showToast('Hub disconnected', 'success');
                    checkStatus();
                } else {
                    showToast(data.message, 'error');
                }
            } catch (e) {
                showToast('Failed to disconnect', 'error');
            } finally {
                clearHubBtn.disabled = false;
            }
        });

        // Utility
        function escapeHtml(text) {
            const div = document.createElement('div');
            div.textContent = text;
            return div.innerHTML;
        }
        function formatSize(bytes) {
            if (bytes < 1024) return bytes + ' B';
            if (bytes < 1024*1024) return (bytes/1024).toFixed(1) + ' KB';
            return (bytes/(1024*1024)).toFixed(1) + ' MB';
        }
        function formatDate(isoDate) {
            try { return new Date(isoDate).toLocaleDateString(); } catch { return isoDate; }
        }

        // Init
        checkStatus();
    </script>
</body>
</html>
"#;

/// CSS stylesheet
pub const STYLE_CSS: &str = r#"/* Additional styles if needed */"#;

/// Get an embedded asset by path
pub fn get_asset(path: &str) -> Option<(&'static str, &'static str)> {
    match path {
        "style.css" => Some((STYLE_CSS, "text/css")),
        _ => None,
    }
}
