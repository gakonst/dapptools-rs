//! ChiselSession
//!
//! This module contains the `ChiselSession` struct, which is the top-level
//! wrapper for a serializable REPL session.

use crate::{prelude::SessionSource, session_source::SessionSourceConfig};
use ethers_solc::Solc;
use eyre::Result;
use foundry_config::SolcReq;
use serde::{Deserialize, Serialize};
use std::path::Path;
use time::{format_description, OffsetDateTime};
use yansi::Paint;

/// A Chisel REPL Session
#[derive(Debug, Serialize, Deserialize)]
pub struct ChiselSession {
    /// The `SessionSource` object that houses the REPL session.
    pub session_source: Option<SessionSource>,
    /// The current session's identifier
    pub id: Option<String>,
}

// ChiselSession Common Associated Functions
impl ChiselSession {
    /// Create a new `ChiselSession` with a specified `solc` version and configuration.
    ///
    /// # Panics
    ///
    /// Panics if there is a failure to install the requested Solc version.
    pub fn new(config: &SessionSourceConfig) -> Self {
        // Solc version precidence
        // - Foundry configuration / `--use` flag
        // - Latest installed version via SVM
        // - Default: 0.8.17
        let solc = Solc::find_or_install_svm_version(
            if let Some(SolcReq::Version(version)) = config.foundry_config.solc.as_ref() {
                let version = format!("{}.{}.{}", version.major, version.minor, version.patch);
                if let Ok(None) = Solc::find_svm_installed_version(&version) {
                    println!(
                        "{}",
                        Paint::green(format!("Installing solidity version {}...", &version))
                    );
                }
                version
            } else {
                // If no version was explicitly set, use the latest SVM version.
                if let Some(version) = Solc::installed_versions().into_iter().max() {
                    version.to_string()
                } else {
                    println!(
                        "{}",
                        Paint::green(
                            "No solidity versions installed! Installing solidity version 0.8.17..."
                        )
                    );
                    String::from("0.8.17")
                }
            },
        );

        // Return initialized ChiselSession with set solc version
        if let Ok(solc) = solc {
            Self { session_source: Some(SessionSource::new(&solc, config)), id: None }
        } else {
            panic!("Failed to install solidity via svm!");
        }
    }

    /// Render the full source code for the current session.
    ///
    /// ### Return
    ///
    /// Returns the full, flattened source code for the current session.
    ///
    /// ### Notes
    ///
    /// This function will not panic, but will return a blank string if the
    /// session's [SessionSource] is None.
    pub fn contract_source(&self) -> String {
        if let Some(source) = &self.session_source {
            source.to_string()
        } else {
            String::default()
        }
    }

    /// Clears the cache directory
    ///
    /// ### WARNING
    ///
    /// This will delete all sessions from the cache.
    /// There is no method of recovering these deleted sessions.
    pub fn clear_cache() -> Result<()> {
        let cache_dir = Self::cache_dir()?;
        for entry in std::fs::read_dir(cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    /// Writes the ChiselSession to a file by serializing it to a JSON string
    ///
    /// ### Returns
    ///
    /// Returns the path of the new cache file
    pub fn write(&mut self) -> Result<String> {
        // Try to create the cache directory
        let cache_dir = Self::cache_dir()?;
        std::fs::create_dir_all(&cache_dir)?;

        let cache_file_name = match self.id.as_ref() {
            Some(id) => {
                // ID is already set- use the existing cache file.
                format!("{}chisel-{}.json", cache_dir, id)
            }
            None => {
                // Get the next session cache ID / file
                let (id, file_name) = Self::next_cached_session()?;
                // Set the session's ID
                self.id = Some(id);
                // Return the new session's cache file name
                file_name
            }
        };

        // Write the current ChiselSession to that file
        let serialized_contents = serde_json::to_string_pretty(self)?;
        std::fs::write(&cache_file_name, serialized_contents)?;

        // Return the full cache file path
        // Ex: /home/user/.foundry/cache/chisel/chisel-0.json
        Ok(cache_file_name)
    }

    /// Get the next default session cache file name
    pub fn next_cached_session() -> Result<(String, String)> {
        let cache_dir = Self::cache_dir()?;
        let mut entries = std::fs::read_dir(&cache_dir)?;

        // If there are no existing cached sessions, just create the first one: "chisel-0.json"
        let mut latest = if let Some(e) = entries.next() {
            e?
        } else {
            return Ok((String::from("0"), format!("{}chisel-0.json", cache_dir)))
        };

        let mut session_num = 1;
        // Get the latest cached session
        for entry in entries {
            let entry = entry?;
            if entry.metadata()?.modified()? >= latest.metadata()?.modified()? {
                latest = entry;
            }

            // Increase session_num counter rather than cloning the iterator and using `.count`
            session_num += 1;
        }

        Ok((format!("{}", session_num), format!("{}chisel-{}.json", cache_dir, session_num)))
    }

    /// The Chisel Cache Directory
    pub fn cache_dir() -> Result<String> {
        let home_dir = dirs::home_dir().ok_or(eyre::eyre!("Failed to grab home directory"))?;
        let home_dir_str =
            home_dir.to_str().ok_or(eyre::eyre!("Failed to convert home directory to string"))?;
        Ok(format!("{}/.foundry/cache/chisel/", home_dir_str))
    }

    /// Create the cache directory if it does not exist
    pub fn create_cache_dir() -> Result<()> {
        let cache_dir = Self::cache_dir()?;
        if !Path::new(&cache_dir).exists() {
            std::fs::create_dir_all(&cache_dir)?;
        }
        Ok(())
    }

    /// Lists all available cached sessions
    pub fn list_sessions() -> Result<Vec<(String, String)>> {
        // Read the cache directory entries
        let cache_dir = Self::cache_dir()?;
        let entries = std::fs::read_dir(&cache_dir)?;

        // For each entry, get the file name and modified time
        let mut sessions = Vec::new();
        for entry in entries {
            let entry = entry?;
            let modified_time = entry.metadata()?.modified()?;
            let file_name = entry.file_name();
            let file_name = file_name
                .into_string()
                .map_err(|e| eyre::eyre!(format!("{}", e.to_string_lossy())))?;
            sessions.push((
                systemtime_strftime(modified_time, "[year]-[month]-[day] [hour]:[minute]:[second]")
                    .unwrap(),
                file_name,
            ));
        }

        if sessions.is_empty() {
            eyre::bail!("No sessions found!")
        } else {
            // Return the list of sessions and their modified times
            Ok(sessions)
        }
    }

    /// Gets the most recent chisel session from the cache dir
    pub fn latest_chached_session() -> Result<String> {
        let cache_dir = Self::cache_dir()?;
        let mut entries = std::fs::read_dir(cache_dir)?;
        let mut latest = entries.next().ok_or(eyre::eyre!("No entries found!"))??;
        for entry in entries {
            let entry = entry?;
            if entry.metadata()?.modified()? > latest.metadata()?.modified()? {
                latest = entry;
            }
        }
        Ok(latest.path().to_str().ok_or(eyre::eyre!("Failed to get session path!"))?.to_string())
    }

    /// Loads a specific ChiselSession from the specified cache file
    pub fn load(id: &str) -> Result<Self> {
        let cache_dir = ChiselSession::cache_dir()?;
        let contents =
            std::fs::read_to_string(Path::new(&format!("{}chisel-{}.json", cache_dir, id)))?;
        let chisel_env: ChiselSession = serde_json::from_str(&contents)?;
        Ok(chisel_env)
    }

    /// Loads the latest ChiselSession from the cache file
    pub fn latest() -> Result<Self> {
        let last_session = Self::latest_chached_session()?;
        let last_session_contents = std::fs::read_to_string(Path::new(&last_session))?;
        let chisel_env: ChiselSession = serde_json::from_str(&last_session_contents)?;
        Ok(chisel_env)
    }
}

/// Generic helper function that attempts to convert a type that has
/// an [Into<OffsetDateTime>] implementation into a formatted date string.
fn systemtime_strftime<T>(dt: T, format: &str) -> Result<String>
where
    T: Into<OffsetDateTime>,
{
    Ok(dt.into().format(&format_description::parse(format)?)?)
}
