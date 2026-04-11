use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read},
    path::Path,
};

use crate::{error::CascError, game::GameType, types::Md5Hash};

// ── KeyValueConfig ─────────────────────────────────────────────────────────────

/// A `key = value [value …]` configuration file (build config, CDN config).
///
/// Lines starting with `#` and blank lines are ignored.  Each key maps to a
/// list of whitespace-separated values on the right-hand side.
#[derive(Debug, Default)]
pub struct KeyValueConfig {
    data: HashMap<String, Vec<String>>,
}

impl KeyValueConfig {
    /// Parse from any `Read` source.
    pub fn from_reader<R: Read>(reader: R) -> Result<Self, CascError> {
        let mut cfg = KeyValueConfig::default();
        for line in BufReader::new(reader).lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let eq = line.find('=').ok_or_else(|| {
                CascError::Config(format!("KeyValueConfig: no '=' in line: {line}"))
            })?;

            let key = line[..eq].trim().to_string();
            let right = line[eq + 1..].trim();
            let vals: Vec<String> = right.split_ascii_whitespace().map(str::to_owned).collect();

            cfg.data.insert(key, vals);
        }
        Ok(cfg)
    }

    /// Look up a key, returning `None` if absent.
    pub fn get(&self, key: &str) -> Option<&Vec<String>> {
        self.data.get(key)
    }

    /// Shorthand for `get(key).and_then(|v| v.first())`.
    pub fn get_first(&self, key: &str) -> Option<&str> {
        self.data.get(key)?.first().map(String::as_str)
    }

    /// Iterate all key-value pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Vec<String>)> {
        self.data.iter().map(|(k, v)| (k.as_str(), v))
    }
}

// ── VerBarConfig ───────────────────────────────────────────────────────────────

/// A pipe-and-bar configuration table, used by `.build.info`, `versions`, and
/// `cdns` responses.
///
/// The first non-comment line defines column headers in `Name!TYPE:SIZE`
/// format.  Subsequent lines are data rows.
#[derive(Debug, Default)]
pub struct VerBarConfig {
    columns: Vec<String>,
    rows: Vec<HashMap<String, String>>,
}

impl VerBarConfig {
    /// Parse from any `Read` source.
    pub fn from_reader<R: Read>(reader: R) -> Result<Self, CascError> {
        let mut cfg = VerBarConfig::default();
        let mut first_row = true;

        for line in BufReader::new(reader).lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let tokens: Vec<&str> = line.split('|').collect();

            if first_row {
                // header row: strip type annotations after '!'
                cfg.columns = tokens
                    .iter()
                    .map(|t| {
                        let name = t.split('!').next().unwrap_or(*t);
                        name.replace(' ', "")
                    })
                    .collect();
                first_row = false;
            } else {
                let mut row = HashMap::new();
                for (col, val) in cfg.columns.iter().zip(tokens.iter()) {
                    row.insert(col.clone(), val.trim().to_string());
                }
                cfg.rows.push(row);
            }
        }

        Ok(cfg)
    }

    /// Return the value of `column` in the first row where `filter_col ==
    /// filter_val`, or in the first row if no filter column exists.
    pub fn get(&self, filter_col: &str, filter_val: &str, column: &str) -> Option<&str> {
        if self.rows.is_empty() {
            return None;
        }

        let has_filter = self.columns.iter().any(|c| c == filter_col);

        if has_filter {
            self.rows.iter().find_map(|row| {
                if row.get(filter_col).map(String::as_str) == Some(filter_val) {
                    row.get(column).map(String::as_str)
                } else {
                    None
                }
            })
        } else {
            self.rows.first()?.get(column).map(String::as_str)
        }
    }

    /// Iterate all rows.
    pub fn rows(&self) -> &[HashMap<String, String>] {
        &self.rows
    }

    /// Number of data rows.
    pub fn count(&self) -> usize {
        self.rows.len()
    }

    /// All values of `column` across every row (skips rows where the column is absent).
    pub fn all_values(&self, column: &str) -> impl Iterator<Item = &str> + use<'_> {
        let col = column.to_owned();
        self.rows
            .iter()
            .filter_map(move |r| r.get(&col).map(String::as_str))
    }
}

// ── CascConfig ─────────────────────────────────────────────────────────────────

/// Parsed configuration for a local CASC installation.
///
/// Loads and cross-references `.build.info`, the build config, and the CDN
/// config from the game's data folder.
#[derive(Debug)]
pub struct CascConfig {
    /// Root of the game installation (parent of the data folder).
    pub base_path: std::path::PathBuf,
    /// Detected game type.
    pub game_type: GameType,
    /// Product UID (e.g. `"wow"`, `"d3"`).
    pub product: String,
    /// Active build configuration (there is usually exactly one).
    pub build: KeyValueConfig,
    /// CDN configuration.
    pub cdn: KeyValueConfig,
}

impl CascConfig {
    /// Read the product UIDs present in a `.build.info` file without fully
    /// loading the installation.
    ///
    /// Returns a list of internal product UIDs (e.g. `"fenris"` for D4,
    /// `"wow"` for World of Warcraft).  Returns an empty vec if the file does
    /// not exist or has no `Product` column.
    pub fn detect_products(base_path: impl AsRef<Path>) -> Vec<String> {
        let path = base_path.as_ref().join(".build.info");
        let Ok(file) = std::fs::File::open(&path) else {
            return vec![];
        };
        let Ok(info) = VerBarConfig::from_reader(file) else {
            return vec![];
        };
        info.all_values("Product")
            .map(std::borrow::ToOwned::to_owned)
            .collect()
    }

    /// Load a local CASC installation.
    ///
    /// `base_path` should be the directory that contains the `.build.info`
    /// file (typically the game's installation root).
    pub fn load_local(base_path: impl AsRef<Path>, product: &str) -> Result<Self, CascError> {
        let base_path = base_path.as_ref().to_path_buf();

        // ── .build.info ────────────────────────────────────────────────────
        let build_info_path = base_path.join(".build.info");
        let build_info =
            VerBarConfig::from_reader(std::fs::File::open(&build_info_path).map_err(|e| {
                CascError::Config(format!(
                    "Cannot open .build.info at {}: {e}",
                    build_info_path.display()
                ))
            })?)?;

        // Resolve the effective product UID:
        //   1. Try the row where Product == product (exact match).
        //   2. If no exact match, try the row whose Product UID *starts with* product
        //      (e.g. user types "fenris", file has "fenris_beta").
        //   3. If still no match, fall back to the first row (single-product installs).
        // In all cases we use the UID actually stored in the file for game-type detection
        // so the caller doesn't need to know Blizzard's internal code names.
        let has_product_col = build_info.columns.iter().any(|c| c == "Product");

        let resolved_product: String = if has_product_col {
            // Exact match first.
            if build_info.get("Product", product, "Product").is_some() {
                product.to_owned()
            } else {
                // Prefix/fallback: pick the first row whose UID starts with `product`,
                // or, if nothing matches at all, use the first row's UID.
                build_info
                    .all_values("Product")
                    .find(|uid| uid.starts_with(product) || product.starts_with(*uid))
                    .or_else(|| build_info.all_values("Product").next())
                    .map_or_else(|| product.to_owned(), std::borrow::ToOwned::to_owned)
            }
        } else {
            // Older format: no Product column, fall back to passed-in product.
            product.to_owned()
        };

        let game_type = GameType::from_uid(&resolved_product)?;

        let data_folder = game_type.data_folder().ok_or_else(|| {
            CascError::Config(format!(
                "Game type {game_type:?} has no known local data folder"
            ))
        })?;

        // Helper: open a two-level hash-keyed config file (aa/bb/aabb…).
        let open_config = |key: &str| -> Result<std::fs::File, CascError> {
            if key.len() < 4 {
                return Err(CascError::Config(format!("config key too short: {key}")));
            }
            let path = base_path
                .join(data_folder)
                .join("config")
                .join(&key[..2])
                .join(&key[2..4])
                .join(key);
            std::fs::File::open(&path).map_err(|e| {
                CascError::Config(format!("Cannot open config {}: {e}", path.display()))
            })
        };

        // ── Build config ───────────────────────────────────────────────────
        let build_key = build_info
            .get("Product", &resolved_product, "BuildKey")
            .or_else(|| {
                build_info
                    .rows()
                    .first()?
                    .get("BuildKey")
                    .map(String::as_str)
            })
            .ok_or_else(|| CascError::Config("BuildKey missing from .build.info".into()))?
            .to_lowercase();

        let build = KeyValueConfig::from_reader(open_config(&build_key)?)?;

        // ── CDN config ─────────────────────────────────────────────────────
        let cdn_key = build_info
            .get("Product", &resolved_product, "CDNKey")
            .or_else(|| build_info.rows().first()?.get("CDNKey").map(String::as_str))
            .ok_or_else(|| CascError::Config("CDNKey missing from .build.info".into()))?
            .to_lowercase();

        let cdn = KeyValueConfig::from_reader(open_config(&cdn_key)?)?;

        Ok(CascConfig {
            base_path,
            game_type,
            product: resolved_product,
            build,
            cdn,
        })
    }

    // ── Build config accessors ─────────────────────────────────────────────

    fn build_hex_key(&self, field: &str) -> Option<Md5Hash> {
        let hex = self.build.get_first(field)?;
        Md5Hash::from_hex(hex)
    }

    /// Content key for the root manifest.
    pub fn root_ckey(&self) -> Option<Md5Hash> {
        self.build_hex_key("root")
    }

    /// Content key for the encoding file.
    pub fn encoding_ckey(&self) -> Option<Md5Hash> {
        self.build_hex_key("encoding")
    }

    /// Encoding key for the encoding file (used to open it before the encoding
    /// table itself is loaded).
    pub fn encoding_ekey(&self) -> Option<Md5Hash> {
        let vals = self.build.get("encoding")?;
        vals.get(1).and_then(|s| Md5Hash::from_hex(s))
    }

    /// Content key for the install manifest.
    pub fn install_ckey(&self) -> Option<Md5Hash> {
        self.build_hex_key("install")
    }

    /// Content key for the download manifest.
    pub fn download_ckey(&self) -> Option<Md5Hash> {
        self.build_hex_key("download")
    }

    /// List of CDN archive IDs.
    pub fn archives(&self) -> &[String] {
        self.cdn
            .get("archives")
            .map_or(&[], std::vec::Vec::as_slice)
    }

    /// Local data folder path (e.g. `<base>/Data/data/`).
    pub fn data_path(&self) -> std::path::PathBuf {
        let data_folder = self.game_type.data_folder().unwrap_or("Data");
        self.base_path.join(data_folder).join("data")
    }

    /// Local config folder path.
    pub fn config_path(&self) -> std::path::PathBuf {
        let data_folder = self.game_type.data_folder().unwrap_or("Data");
        self.base_path.join(data_folder).join("config")
    }

    /// Local indices folder path.
    pub fn indices_path(&self) -> std::path::PathBuf {
        let data_folder = self.game_type.data_folder().unwrap_or("Data");
        self.base_path.join(data_folder).join("indices")
    }
}
