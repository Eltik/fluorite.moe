use crate::extraction::lua::{TreeDecryptResult, decrypt_lua, decrypt_tree, is_lua_metadata};
use std::fs;

/// `LuaScripts/UI/Panels/RootConfig.lua` as stored in the game's Lua group.
const ROOT_CONFIG: &[u8] = b"71KyC+WgcFKhMS/8Uma4BWEd2PgoCF+lG1vsyNN78+E=";
/// `LuaScripts/Const/DialogConst.lua`: two words holding a single newline.
const DIALOG_CONST: &[u8] = b"opkmRFNE6wo=\n";

#[test]
fn decrypts_stored_scripts() {
    assert_eq!(
        decrypt_lua(ROOT_CONFIG).unwrap(),
        b"config = {\r\n    \n    \r\n}\r\n"
    );
    assert_eq!(decrypt_lua(DIALOG_CONST).unwrap(), b"\n");
}

#[test]
fn leaves_plain_or_damaged_input_alone() {
    assert_eq!(decrypt_lua(b"config = {}\n"), None);
    assert_eq!(decrypt_lua(b""), None);
    // Valid base64 whose decrypted length word does not fit the payload.
    let mut damaged = ROOT_CONFIG.to_vec();
    damaged[0] = b'A';
    assert_eq!(decrypt_lua(&damaged), None);
}

#[test]
fn recognizes_lua_group_metadata() {
    assert!(is_lua_metadata("Group: Lua (19E3AE45)\nVersion: 3\n"));
    assert!(!is_lua_metadata("Group: LuaExtra (00000000)\n"));
    assert!(!is_lua_metadata("Group: Table (42A8FCA6)\n"));
    assert!(!is_lua_metadata(""));
}

#[test]
fn decrypts_tree_in_place_once() {
    let dir = std::env::temp_dir().join(format!("fluorite-lua-{}", std::process::id()));
    fs::create_dir_all(dir.join("LuaScripts/UI")).unwrap();
    fs::write(dir.join("LuaScripts/UI/RootConfig.lua"), ROOT_CONFIG).unwrap();
    fs::write(dir.join("LuaScripts/Plain.lua"), "return 1\n").unwrap();
    fs::write(dir.join("notes.md"), ROOT_CONFIG).unwrap();

    let first = decrypt_tree(&dir).unwrap();
    assert_eq!(
        first,
        TreeDecryptResult {
            decrypted: 1,
            write_errors: 0
        }
    );
    assert_eq!(
        fs::read(dir.join("LuaScripts/UI/RootConfig.lua")).unwrap(),
        b"config = {\r\n    \n    \r\n}\r\n"
    );
    assert_eq!(fs::read(dir.join("notes.md")).unwrap(), ROOT_CONFIG);
    assert_eq!(decrypt_tree(&dir).unwrap(), TreeDecryptResult::default());
    fs::remove_dir_all(dir).unwrap();
}
