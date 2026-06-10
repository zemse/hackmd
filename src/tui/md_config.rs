use serde::Deserialize;

#[derive(Default, Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Config {
    pub theme: Option<String>,
    pub width: Option<u16>,
    pub line_numbers: Option<bool>,
}

pub fn load() -> Config {
    let path = match dirs::config_dir() {
        Some(d) => d.join("md").join("config.toml"),
        None => return Config::default(),
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    toml::from_str(&text).unwrap_or_default()
}
