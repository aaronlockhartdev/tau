//! The rules that make `provider/key` a name for exactly one endpoint (#354).
//!
//! §6.7's routing, the GUI's model menu, a session record's model field,
//! `[thinking].levels` keys and an agent type's frontmatter all carry the id as
//! one string, so the string has to answer for one endpoint and no more. Each
//! rule below removes a shape where it answers for more than one, or for none.

use super::{Config, LoadError};

/// A model id's provider prefix: the segment before the first `/`, and `None`
/// for a bare key.
fn prefix_of(id: &str) -> Option<&str> {
    id.split_once('/').map(|(prefix, _)| prefix)
}

/// Refuse a config whose id spelling is ambiguous at load, naming the entry the
/// user's edit has to change.
pub(crate) fn validate(config: &Config) -> Result<(), LoadError> {
    for name in config.providers.keys() {
        if name.contains('/') {
            return Err(LoadError::ProviderNameHasSlash {
                provider: name.clone(),
            });
        }
    }
    for (name, provider) in &config.providers {
        for model in provider.models.keys() {
            // Routed by its prefix, that id reaches `<named>`'s model past the
            // slash, so the key as declared is reachable by no id.
            if let Some(named) =
                prefix_of(model).filter(|prefix| config.providers.contains_key(*prefix))
            {
                return Err(LoadError::UnreachableModelKey {
                    provider: name.clone(),
                    model: model.clone(),
                    named: named.to_owned(),
                });
            }
        }
    }
    for key in config.thinking.levels.keys() {
        // A prefixed level is read when its id resolves; a prefix that names no
        // provider means the entry is never read.
        if let Some(prefix) =
            prefix_of(key).filter(|prefix| !config.providers.contains_key(*prefix))
        {
            return Err(LoadError::LevelPrefixUnknown {
                key: key.clone(),
                prefix: prefix.to_owned(),
            });
        }
    }
    Ok(())
}
