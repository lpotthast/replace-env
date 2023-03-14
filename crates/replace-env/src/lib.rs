#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]

pub use derive_replace_env::ReplaceEnv;
use tracing::warn;

pub struct Metadata {
    pub secret: bool,
}

pub trait ReplaceEnv {
    fn replace_env(self, metadata: Metadata) -> Self;
}

impl ReplaceEnv for String {
    fn replace_env(mut self, metadata: Metadata) -> Self {
        replace_env_in_string(&mut self, metadata);
        self
    }
}

impl ReplaceEnv for Option<String> {
    fn replace_env(mut self, metadata: Metadata) -> Self {
        if let Some(inner) = &mut self {
            replace_env_in_string(inner, metadata);
        }
        self
    }
}

/// Checks if the given string starts with "${", ends with "}" and contains at least one ":". Only then modifies the given `string` by
/// trying to obtain the value of the environment variable denoted by the substring after "${" and before the first ":".
/// If that value could be determined, replaces the whole string with that value.
/// If that value could not be determined, replaces the whole string with the default value,
/// denoted by the substring starting after the first ":" end ending before "}".
fn replace_env_in_string(string: &mut String, metadata: Metadata) {
    if string.starts_with(['$', '{']) && string.ends_with('}') {
        if let Some((env_name, default_value)) = string.split_once(':') {
            let env_name = &env_name[2..env_name.len()]; // Remove leading "${".
            let default_value = &default_value[0..default_value.len() - 1]; // Remove trailing "}".
            match std::env::var(env_name) {
                Ok(env_value) => {
                    string.clear();
                    string.push_str(env_value.as_str());
                }
                Err(var_error) => {
                    match var_error {
                        std::env::VarError::NotPresent => match metadata.secret {
                            false => warn!("ENV variable \"{env_name}\" not present. Using default: \"{default_value}\""),
                            true => warn!("ENV variable \"{env_name}\" not present. Using secret default."),
                        },
                        std::env::VarError::NotUnicode(_) => match metadata.secret {
                            false => warn!("ENV variable \"{env_name}\" doest not contain valid unicode! Using default: \"{default_value}\""),
                            true => warn!("ENV variable \"{env_name}\" doest not contain valid unicode! Using secret default."),
                        },
                    }
                    let default_string = default_value.to_string();
                    string.clear();
                    string.push_str(default_string.as_str());
                }
            }
        }
    }
}
