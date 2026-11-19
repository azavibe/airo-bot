/// The app's own commands (`tauri::generate_handler!` in `src/lib.rs`).
///
/// The window shows pages Tauri treats as remote (the local Airobot server on
/// `http://127.0.0.1:<port>` and the hosted site), and remote pages may only
/// call a command a capability grants. Declaring the commands here makes
/// Tauri generate an `allow-<command>` permission for each; the capabilities
/// in `capabilities/` grant them. A command missing from this list, or from
/// the capabilities, is rejected at run time with "not allowed by ACL"
/// (the settings panel then never finds the AI tools), so the
/// `commands_are_granted_to_the_window` test checks all three agree.
const COMMANDS: &[&str] = &[
    "local_bot_status",
    "local_bot_pair",
    "local_bot_configure",
    "local_bot_set_chat",
    "local_bot_forget",
    "desktop_info",
    "desktop_set_mode",
    "open_external",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}
