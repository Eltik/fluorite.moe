//! The read-only sub-trees of the extraction output.
//!
//! A root is a named directory below the output directory: the API serves them, the
//! unpacker writes them and the publisher mirrors them into the bucket. The mapping is
//! the one the whole project shares, so it lives here and not in the API.

use std::path::{Path, PathBuf};

/// A named read-only sub-tree of the output directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Root {
    Textures,
    Meshes,
    Gamedata,
    Tables,
    Jsondata,
    Lua,
    Audio,
    Models,
}

impl Root {
    pub const ALL: [Root; 8] = [
        Root::Textures,
        Root::Meshes,
        Root::Gamedata,
        Root::Tables,
        Root::Jsondata,
        Root::Lua,
        Root::Audio,
        Root::Models,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Root::Textures => "textures",
            Root::Meshes => "meshes",
            Root::Gamedata => "gamedata",
            Root::Tables => "tables",
            Root::Jsondata => "jsondata",
            Root::Lua => "lua",
            Root::Audio => "audio",
            Root::Models => "models",
        }
    }

    /// Directory relative to the output dir.
    pub fn rel_dir(self) -> &'static str {
        match self {
            Root::Textures => "textures",
            Root::Meshes => "meshes",
            Root::Gamedata => "gamedata",
            Root::Tables => "decrypted/Table/TableCfg",
            Root::Jsondata => "decrypted/JsonData/Json",
            Root::Lua => "decrypted/Lua/LuaScripts",
            Root::Audio => "decrypted/Audio",
            Root::Models => "models",
        }
    }

    /// The root with that name, if there is one.
    pub fn parse(name: &str) -> Option<Root> {
        Root::ALL.into_iter().find(|r| r.name() == name)
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn dir(self, output: &Path) -> PathBuf {
        output.join(self.rel_dir())
    }
}
