pub mod cargo_path_utils;

use std::{fmt::Display, ops::Deref, str::FromStr};

use camino::Utf8PathBuf;
use serde::{de, Deserialize, Deserializer, Serialize};
use thiserror::Error;
use tracing::debug;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LibPathSet {
    path: Utf8PathBuf,
}

impl LibPathSet {
    pub fn new(path: impl Into<Utf8PathBuf>) -> Self {
        debug!("Creating path");
        Self { path: path.into() }
    }

    pub fn library_path(&self) -> Utf8PathBuf {
        self.path.clone()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct HotReloadOptions {
    pub manifest_path: Option<Utf8PathBuf>,
    pub package: Option<String>,
    pub example: Option<String>,
    pub lib_name: Option<String>,
    pub watch_folders: Vec<Utf8PathBuf>,
    pub target_folder: Option<Utf8PathBuf>,
    pub features: Vec<String>,
    pub build_target: Option<Target>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, Hash, PartialEq, Eq)]
pub enum PackageOrExample {
    #[default]
    DefaulPackage,
    Package(String),
    Example(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TargetBuildSettings {
    pub package_or_example: PackageOrExample,
    pub features: Vec<String>,
    pub asset_folders: Vec<camino::Utf8PathBuf>,
    pub code_watch_folders: Vec<camino::Utf8PathBuf>,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum Target {
    Linux,
    LinuxArm,
    Windows,
    Mac,
    MacArm,
    Android,
    IOS,
}

impl Target {
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "linux") {
            if cfg!(target_arch = "aarch64") {
                Some(Self::LinuxArm)
            } else {
                Some(Self::Linux)
            }
        } else if cfg!(target_os = "windows") {
            Some(Self::Windows)
        } else if cfg!(target_os = "macos") {
            if cfg!(target_arch = "aarch64") {
                Some(Self::MacArm)
            } else {
                Some(Self::Mac)
            }
        } else {
            None
        }
    }
}

impl Serialize for Target {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self.to_static())
    }
}

impl<'de> Deserialize<'de> for Target {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(de::Error::custom)
    }
}

impl Target {
    pub const fn to_static(self) -> &'static str {
        match self {
            Target::Linux => "x86_64-unknown-linux-gnu",
            Target::LinuxArm => "aarch64-unknown-linux-gnu",
            Target::Windows => {
                if cfg!(windows) {
                    "x86_64-pc-windows-msvc"
                } else {
                    "x86_64-pc-windows-gnu"
                }
            }
            Target::Mac => "x86_64-apple-darwin",
            Target::MacArm => "aarch64-apple-darwin",
            Target::Android => "aarch64-linux-android",
            Target::IOS => "aarch64-apple-ios",
        }
    }

    pub fn dynamic_lib_extension(&self) -> &'static str {
        match self {
            Target::Windows => "dll",
            Target::Mac => "dylib",
            Target::MacArm => "dylib",
            Target::IOS => "dylib",
            _ => "so"
        }
    }

    pub fn as_str(&self) -> &'static str {
        self.to_static()
    }
}

impl Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

impl Deref for Target {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum TargetParseError {
    #[error("Couldn't Parse Target")]
    InvalidTarget,
}

impl FromStr for Target {
    type Err = TargetParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim().to_lowercase();
        if s.contains("windows") {
            Ok(Self::Windows)
        } else if s.contains("android") {
            Ok(Self::Android)
        } else if s.contains("linux") {
            if s.contains("arm") || s.contains("aarch") {
                Ok(Self::LinuxArm)
            } else {
                Ok(Self::Linux)
            }
        } else if s.contains("darwin") || s.contains("mac") {
            if s.contains("arm") || s.contains("aarch") {
                Ok(Self::MacArm)
            } else {
                Ok(Self::Mac)
            }
        } else if s.contains("ios") {
            Ok(Self::IOS)
        } else {
            Err(TargetParseError::InvalidTarget)
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum HotReloadMessage {
    InitialState {
        id: uuid::Uuid,
        root_lib: Option<Utf8PathBuf>,
        libraries: Vec<(Utf8PathBuf, [u8; 32])>,
        assets: Vec<(Utf8PathBuf, [u8; 32])>,
    },
    RootLibPath(Utf8PathBuf),
    UpdatedLibs(Utf8PathBuf, [u8; 32]),
    UpdatedAssets(Utf8PathBuf, [u8; 32]),
    KeepAlive,
}
