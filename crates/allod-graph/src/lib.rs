pub mod error;
pub mod fed;
pub mod flows;
pub mod md;
pub mod ops;
pub mod profiles;
#[cfg(feature = "native")]
pub mod repo;
pub mod schema;
pub use error::AllodError;

/// Point ALLOD_KEYS_DIR at a per-process temp dir so tests never write
/// to the real XDG data dir. Safe to call from every test; first call wins.
pub fn hermetic_keys_for_tests() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("allod-test-keys-{}", std::process::id()));
        std::env::set_var("ALLOD_KEYS_DIR", &dir);
    });
}
