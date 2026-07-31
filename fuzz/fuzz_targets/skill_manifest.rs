#![no_main]

use libfuzzer_sys::fuzz_target;
use polkagent_skill::manifest::SkillManifest;

fuzz_target!(|data: &[u8]| {
    // FZ-07: Fuzz SkillManifest TOML parsing.
    //
    // SkillManifest::from_toml performs both deserialization *and* semantic
    // validation (name format, semver version, dependency version reqs).
    // Neither path may ever panic.

    if let Ok(s) = std::str::from_utf8(data) {
        // Path 1: full validated parse (includes name/version/dep validation).
        let _ = SkillManifest::from_toml(s);

        // Path 2: raw TOML deserialization without the validation layer.
        // This exercises the serde path independently.
        let _ = toml::from_str::<SkillManifest>(s);

        // Path 3: deserialize as a generic TOML value — covers any parser
        // code paths not reached by the typed deserialization.
        let _ = toml::from_str::<toml::Value>(s);
    }

    // If the manifest parses and validates successfully, exercise all
    // derived methods — none of these should panic.
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(manifest) = SkillManifest::from_toml(s) {
            // id() parses name + version into a SkillId.
            let _ = manifest.id();

            // version() parses the version string.
            let _ = manifest.version();

            // Display the skill section fields — must not panic.
            let _ = manifest.skill.name.len();
            let _ = manifest.skill.version.len();
            let _ = manifest.skill.description.len();
            let _ = manifest.capabilities.required_grants.len();
            let _ = manifest.capabilities.tools.len();
            let _ = manifest.prompts.system.len();
            let _ = manifest.dependencies.len();
            let _ = manifest.config.len();

            // Re-serialize to JSON — must not panic.
            let _ = serde_json::to_string(&manifest);
        }
    }
});
