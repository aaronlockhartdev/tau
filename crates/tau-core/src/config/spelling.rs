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
            // The bare id is spelled in `[generation].default_model`, an agent
            // type's `model:`, and `[om].om_model`; routed by its prefix it names
            // `<named>`'s model past the slash, not this key — which is
            // addressable only as `<name>/<model>`. One string, two endpoints.
            if let Some(named) =
                prefix_of(model).filter(|prefix| config.providers.contains_key(*prefix))
            {
                return Err(LoadError::ShadowedModelKey {
                    provider: name.clone(),
                    model: model.clone(),
                    named: named.to_owned(),
                });
            }
        }
    }
    for key in config.thinking.levels.keys() {
        // derive_turn looks a level up by the key a model id routes to, with a
        // configured provider's prefix stripped; a prefixed key no provider
        // declares as a model is read by nothing.
        if key.contains('/')
            && !config
                .providers
                .values()
                .any(|provider| provider.models.contains_key(key))
        {
            return Err(LoadError::LevelUndeclared { key: key.clone() });
        }
    }
    Ok(())
}
