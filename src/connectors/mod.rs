pub mod local_files;
pub mod registry;
pub mod traits;
pub mod web_clip;

pub use local_files::LocalFileConnector;
pub use registry::ConnectorRegistry;
pub use web_clip::WebClipReceiver;
