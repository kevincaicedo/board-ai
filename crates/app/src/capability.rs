//! The window may call exactly the commands the shell registers (LLD §9). Every command in
//! `build.rs`'s manifest is registered in `main.rs` and allowed in the capability, except
//! `dev_say`, which debug builds grant at runtime; the capability allows nothing else. A command
//! missing from the capability is refused by Tauri's ACL only when the window calls it, so this
//! catches it before the app runs.

const BUILD: &str = include_str!("../build.rs");
const MAIN: &str = include_str!("main.rs");
const CAPABILITY: &str = include_str!("../capabilities/default.json");

/// Granted at runtime in debug builds (`dev::grant`), never in the capability.
const RUNTIME_GRANTED: &str = "dev_say";

/// The quoted names in `build.rs`'s `AppManifest::new().commands(&[…])`.
fn manifest() -> Vec<&'static str> {
    let Some((_, list)) = BUILD.split_once(".commands(&[") else {
        return Vec::new();
    };
    let list = list.split_once("])").map_or("", |(inside, _)| inside);
    list.split(',')
        .map(|item| item.trim().trim_matches('"'))
        .filter(|name| !name.is_empty())
        .collect()
}

/// The last path segment of each handler in `generate_handler![…]`.
fn registered() -> Vec<&'static str> {
    let Some((_, list)) = MAIN.split_once("generate_handler![") else {
        return Vec::new();
    };
    // The list ends the `invoke_handler(…)` call; a `#[cfg(…)]` inside it has its own `]`.
    let list = list.split_once("])").map_or("", |(inside, _)| inside);
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.trim_end_matches(',').rsplit("::").next())
        .collect()
}

/// `allow-setup-open-vault` → `setup_open_vault`, for each permission in the capability.
fn allowed() -> Vec<String> {
    CAPABILITY
        .split('"')
        .filter_map(|text| text.strip_prefix("allow-"))
        .map(|name| name.replace('-', "_"))
        .collect()
}

#[test]
fn every_command_is_registered_and_allowed_and_nothing_else_is() {
    let manifest = manifest();
    assert!(manifest.len() > 3, "the manifest was found: {manifest:?}");
    let registered = registered();
    let allowed = allowed();
    for command in &manifest {
        assert!(
            registered.contains(command),
            "{command} is not registered in main.rs"
        );
        if *command != RUNTIME_GRANTED {
            assert!(
                allowed.iter().any(|name| name == command),
                "{command} is not allowed in capabilities/default.json"
            );
        }
    }
    for name in &allowed {
        assert!(
            manifest.contains(&name.as_str()),
            "the capability allows {name}, which build.rs doesn't declare"
        );
    }
    assert!(!allowed.iter().any(|name| name == RUNTIME_GRANTED));
}
