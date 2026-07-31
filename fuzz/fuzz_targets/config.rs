#![no_main]

use libfuzzer_sys::fuzz_target;
use polkagent_config::schema::Config;
use polkagent_config::ConfigLoader;
use std::io::Write;

fuzz_target!(|data: &[u8]| {
    // Attempt 1: interpret bytes as TOML and deserialize into Config.
    if let Ok(s) = std::str::from_utf8(data) {
        // Direct TOML deserialization — must never panic.
        let _ = toml::from_str::<Config>(s);

        // Also try partial TOML → toml::Value → merge path.
        let _ = toml::from_str::<toml::Value>(s);

        // ConfigLoader with extra_toml overlay — must never panic.
        let _ = ConfigLoader::new().with_extra_toml(s).load();
    }

    // Attempt 2: write bytes to a temp file and try load_from_file.
    // This exercises the file-reading path with arbitrary content.
    if let Ok(dir) = tempfile::tempdir() {
        let file_path = dir.path().join("polkagent.toml");
        if let Ok(mut f) = std::fs::File::create(&file_path) {
            // Ignore write errors — we just want to test the loader.
            let _ = f.write_all(data);
            drop(f);

            let loader = ConfigLoader::new();
            let _ = loader.load_from_file(&file_path);
        }
    }

    // Attempt 3: try the merge function with fuzzed TOML on both sides.
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(overlay) = toml::from_str::<Config>(s) {
            let base = Config::default();
            let _ = polkagent_config::merge(base, overlay);
        }
    }
});
