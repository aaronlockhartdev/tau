use super::*;

mod layers;
mod model_facts;
mod routing;
mod spelling;

/// The two layers written to temp dirs, the shape every leg below loads.
fn load_from(system: &str, project: &str) -> Result<Config, LoadError> {
    let io = |path: std::path::PathBuf, e: std::io::Error| LoadError::Io { path, source: e };
    let (sys_dir, proj_dir) = (
        tempfile::tempdir().map_err(|e| io("<tempdir>".into(), e))?,
        tempfile::tempdir().map_err(|e| io("<tempdir>".into(), e))?,
    );
    if !system.is_empty() {
        let path = sys_dir.path().join("config.toml");
        std::fs::write(&path, system).map_err(|e| io(path, e))?;
    }
    if !project.is_empty() {
        let path = proj_dir.path().join("config.toml");
        std::fs::write(&path, project).map_err(|e| io(path, e))?;
    }
    load(sys_dir.path(), proj_dir.path())
}
/// The §6.7 routing set: two distinct providers, so a prefix has something to
/// select and a shared key has something to refuse to guess between. `aaa` sorts
/// first, so every assertion below that lands on `zzz` proves the route is not
/// the map's order (#336).
fn routing_config() -> Config {
    let mut aaa = crate::config::Provider::with_model("http://127.0.0.1:9/aaa/v1", "solo");
    aaa.models.insert("shared".to_owned(), ModelDef::default());
    let mut zzz = crate::config::Provider::with_model("http://127.0.0.1:9/zzz/v1", "zzz-model");
    zzz.models.insert("shared".to_owned(), ModelDef::default());
    Config {
        providers: BTreeMap::from([("aaa".to_owned(), aaa), ("zzz".to_owned(), zzz)]),
        ..Default::default()
    }
}
