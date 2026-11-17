//! Presets: named snapshots of every parameter except Bypass, stored as RON.
//!
//! Factory presets are compiled in; user presets live in the XDG config dir
//! (`~/.config/voice-changer/presets/*.ron`).

use crate::VcParams;
use nice_plug::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Parameters that are global toggles rather than part of a "voice".
const EXCLUDED_IDS: &[&str] = &[
    "bypass",
    "mute",
    "denoise",
    "voice_only",
    "gate_floor",
    "auto_level",
    "in_gain",
    "out_gain",
    "ai_speaker",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Preset {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Plain (un-normalized) values keyed by parameter id. Bools are 0/1,
    /// enums are their variant index.
    pub values: BTreeMap<String, f32>,
    /// AI voice model path, if the preset uses one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_voice: Option<String>,
}

impl Preset {
    /// Snapshot the current parameter values.
    pub fn capture(params: &VcParams, name: impl Into<String>) -> Self {
        let mut values = BTreeMap::new();
        for (id, ptr, _group) in params.param_map() {
            if EXCLUDED_IDS.contains(&id.as_str()) {
                continue;
            }
            // SAFETY: `ptr` points into `params`, which is alive for this call.
            let plain = unsafe { ptr.unmodulated_plain_value() };
            values.insert(id, plain);
        }
        let voice = params.ai_voice();
        Self {
            name: name.into(),
            description: String::new(),
            values,
            ai_voice: if voice.is_empty() { None } else { Some(voice) },
        }
    }

    /// Push this preset into `params`, routing each change through `set`
    /// (so the plugin can go via the host and the app can write directly).
    /// Unknown ids are ignored; parameters missing from the preset keep
    /// their current value.
    pub fn apply(&self, params: &VcParams, mut set: impl FnMut(ParamPtr, f32)) {
        if let Some(voice) = &self.ai_voice {
            params.set_ai_voice(voice);
        } else if self.values.contains_key("ai_on") {
            // A preset that explicitly sets AI off also clears the voice.
            if self.values.get("ai_on").copied().unwrap_or(0.0) < 0.5 {
                params.set_ai_voice("");
            }
        }
        for (id, ptr, _group) in params.param_map() {
            let Some(&plain) = self.values.get(&id) else {
                continue;
            };
            if EXCLUDED_IDS.contains(&id.as_str()) {
                continue;
            }
            // SAFETY: as in `capture`.
            let normalized = unsafe { ptr.preview_normalized(plain) };
            set(ptr, normalized);
        }
    }

    /// Whether `params` still hold this preset's values, ignoring the
    /// settings presets never touch. Lets the UI avoid naming a preset that
    /// is not actually in effect any more.
    pub fn matches(&self, params: &VcParams) -> bool {
        let current = Preset::capture(params, &self.name);
        self.values.iter().all(|(id, want)| {
            EXCLUDED_IDS.contains(&id.as_str())
                || current
                    .values
                    .get(id)
                    .is_some_and(|got| (got - want).abs() < 1e-3)
        })
    }

    /// Apply through a GUI `ParamSetter` (plugin editor or app window).
    pub fn apply_with_setter(&self, params: &VcParams, setter: &ParamSetter) {
        self.apply(params, |ptr, normalized| {
            // SAFETY: `ptr` is valid for the lifetime of `params`.
            unsafe {
                setter.raw_context.raw_begin_set_parameter(ptr);
                setter
                    .raw_context
                    .raw_set_parameter_normalized(ptr, normalized);
                setter.raw_context.raw_end_set_parameter(ptr);
            }
        });
    }

    /// Apply directly with no host involved (standalone startup, CLI).
    pub fn apply_direct(&self, params: &VcParams, sample_rate: f32) {
        self.apply(params, |ptr, normalized| {
            // SAFETY: `ptr` is valid for the lifetime of `params`.
            unsafe {
                ptr._internal_set_normalized_value(normalized);
                ptr._internal_update_smoother(sample_rate, true);
            }
        });
    }

    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default().struct_names(false))
            .expect("preset serializes")
    }

    pub fn from_ron(text: &str) -> Result<Self, ron::error::SpannedError> {
        ron::from_str(text)
    }
}

/// Built-in starting points.
pub fn factory_presets() -> Vec<Preset> {
    const SOURCES: &[&str] = &[
        include_str!("../presets/natural.ron"),
        include_str!("../presets/man.ron"),
        include_str!("../presets/woman.ron"),
        include_str!("../presets/baritone.ron"),
        include_str!("../presets/bass.ron"),
        include_str!("../presets/deep.ron"),
        include_str!("../presets/demon.ron"),
        include_str!("../presets/monster.ron"),
        include_str!("../presets/chipmunk.ron"),
        include_str!("../presets/robot.ron"),
        include_str!("../presets/radio.ron"),
        include_str!("../presets/cave.ron"),
        include_str!("../presets/announcer.ron"),
    ];
    SOURCES
        .iter()
        .map(|s| Preset::from_ron(s).expect("factory preset parses"))
        .collect()
}

#[derive(Debug, Clone)]
pub struct PresetEntry {
    pub preset: Preset,
    /// `None` for factory presets.
    pub path: Option<PathBuf>,
}

impl PresetEntry {
    pub fn is_factory(&self) -> bool {
        self.path.is_none()
    }
}

/// `~/.config/voice-changer`. `None` if no home directory can be found.
pub fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "echo", "voice-changer")
        .map(|d| d.config_dir().to_path_buf())
}

/// Where user presets live. `None` if no home directory can be found.
pub fn user_preset_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("presets"))
}

/// Factory presets followed by user presets sorted by name.
pub fn list_presets() -> Vec<PresetEntry> {
    let mut entries: Vec<PresetEntry> = factory_presets()
        .into_iter()
        .map(|preset| PresetEntry { preset, path: None })
        .collect();
    if let Some(dir) = user_preset_dir()
        && let Ok(read) = std::fs::read_dir(&dir)
    {
        let mut user: Vec<PresetEntry> = read
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "ron"))
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                let preset = Preset::from_ron(&text)
                    .map_err(|err| log::warn!("skipping preset {:?}: {err}", e.path()))
                    .ok()?;
                Some(PresetEntry {
                    preset,
                    path: Some(e.path()),
                })
            })
            .collect();
        user.sort_by(|a, b| a.preset.name.cmp(&b.preset.name));
        entries.extend(user);
    }
    entries
}

fn file_name_for(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s.trim_matches('_');
    format!("{}.ron", if s.is_empty() { "preset" } else { s })
}

fn preset_dir() -> anyhow::Result<PathBuf> {
    user_preset_dir().ok_or_else(|| anyhow::anyhow!("no settings folder"))
}

/// Whether a built-in preset has this name (those cannot be replaced).
pub fn is_factory_name(name: &str) -> bool {
    factory_presets()
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

/// Write a user preset; returns the file it was saved to. A user preset of
/// the same name is replaced.
pub fn save_preset(preset: &Preset) -> anyhow::Result<PathBuf> {
    save_preset_in(&preset_dir()?, preset)
}

fn save_preset_in(dir: &Path, preset: &Preset) -> anyhow::Result<PathBuf> {
    if preset.name.trim().is_empty() {
        anyhow::bail!("a preset needs a name");
    }
    if is_factory_name(&preset.name) {
        anyhow::bail!(
            "“{}” is a built-in preset; pick another name",
            preset.name.trim()
        );
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(file_name_for(&preset.name));
    std::fs::write(&path, preset.to_ron())?;
    Ok(path)
}

/// Give the user preset stored at `path` another name. Fails when a preset
/// of that name exists already.
pub fn rename_preset(path: &Path, new_name: &str) -> anyhow::Result<PathBuf> {
    rename_preset_in(&preset_dir()?, path, new_name)
}

fn rename_preset_in(dir: &Path, path: &Path, new_name: &str) -> anyhow::Result<PathBuf> {
    let new_name = new_name.trim();
    let mut preset = Preset::from_ron(&std::fs::read_to_string(path)?)
        .map_err(|e| anyhow::anyhow!("that preset cannot be read: {e}"))?;
    if preset.name == new_name {
        return Ok(path.to_path_buf());
    }
    let target = dir.join(file_name_for(new_name));
    if target.exists() && target != path {
        anyhow::bail!("there is already a preset called “{new_name}”");
    }
    preset.name = new_name.to_string();
    let saved = save_preset_in(dir, &preset)?;
    if saved != path {
        std::fs::remove_file(path)?;
    }
    Ok(saved)
}

/// Add a preset file someone shared. Returns the name it has now: a number
/// is added when the name is taken, so nothing is ever overwritten.
pub fn import_preset(file: &Path) -> anyhow::Result<String> {
    import_preset_into(&preset_dir()?, file)
}

fn import_preset_into(dir: &Path, file: &Path) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| anyhow::anyhow!("that file cannot be read: {e}"))?;
    let mut preset = Preset::from_ron(&text)
        .map_err(|_| anyhow::anyhow!("that file is not a Voice Changer preset"))?;
    let base = if preset.name.trim().is_empty() {
        file.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Imported".into())
    } else {
        preset.name.trim().to_string()
    };
    let taken = |name: &str| is_factory_name(name) || dir.join(file_name_for(name)).exists();
    let mut name = base.clone();
    let mut n = 2;
    while taken(&name) {
        name = format!("{base} {n}");
        n += 1;
    }
    preset.name = name.clone();
    save_preset_in(dir, &preset)?;
    Ok(name)
}

/// Write a preset to a file of the user's choosing.
pub fn export_preset(preset: &Preset, dest: &Path) -> anyhow::Result<()> {
    std::fs::write(dest, preset.to_ron())?;
    Ok(())
}

/// The file name a preset would be exported under.
pub fn export_file_name(name: &str) -> String {
    file_name_for(name)
}

pub fn delete_preset(path: &Path) -> anyhow::Result<()> {
    std::fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vc-presets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample(name: &str) -> Preset {
        let mut p = factory_presets().remove(0);
        p.name = name.to_string();
        p
    }

    #[test]
    fn built_in_names_cannot_be_taken() {
        let dir = temp_dir("factory");
        let name = factory_presets()[0].name.clone();
        assert!(save_preset_in(&dir, &sample(&name)).is_err());
        assert!(save_preset_in(&dir, &sample("  ")).is_err());
        assert!(save_preset_in(&dir, &sample("Mine")).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn rename_moves_the_file_and_refuses_a_taken_name() {
        let dir = temp_dir("rename");
        let a = save_preset_in(&dir, &sample("First")).unwrap();
        save_preset_in(&dir, &sample("Second")).unwrap();
        assert!(rename_preset_in(&dir, &a, "Second").is_err());
        assert!(a.exists(), "a refused rename leaves the preset alone");
        let b = rename_preset_in(&dir, &a, "Third").unwrap();
        assert!(!a.exists() && b.exists());
        let back = Preset::from_ron(&std::fs::read_to_string(&b).unwrap()).unwrap();
        assert_eq!(back.name, "Third");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn import_never_overwrites_and_rejects_other_files() {
        let dir = temp_dir("import");
        let shared = dir.join("shared.ron");
        export_preset(&sample("Stage"), &shared).unwrap();
        let user = dir.join("user");
        assert_eq!(import_preset_into(&user, &shared).unwrap(), "Stage");
        assert_eq!(import_preset_into(&user, &shared).unwrap(), "Stage 2");
        let junk = dir.join("junk.ron");
        std::fs::write(&junk, "not a preset").unwrap();
        assert!(import_preset_into(&user, &junk).is_err());
        assert_eq!(std::fs::read_dir(&user).unwrap().count(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn factory_presets_parse_and_only_use_known_ids() {
        let params = VcParams::default();
        let known: Vec<String> = params
            .param_map()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        let presets = factory_presets();
        assert!(presets.len() >= 5);
        for p in &presets {
            for id in p.values.keys() {
                assert!(known.contains(id), "preset {} uses unknown id {id}", p.name);
            }
        }
    }

    #[test]
    fn capture_apply_roundtrip() {
        let params = VcParams::default();
        let mut deep = factory_presets()
            .into_iter()
            .find(|p| p.name == "Deep")
            .unwrap();
        deep.apply_direct(&params, 48_000.0);
        assert!((params.pitch.value() - deep.values["pitch"]).abs() < 1e-4);
        let captured = Preset::capture(&params, "copy");
        assert!(!captured.values.contains_key("bypass"));
        for (id, v) in &deep.values {
            if EXCLUDED_IDS.contains(&id.as_str()) {
                continue;
            }
            assert!(
                (captured.values[id] - v).abs() < 1e-3,
                "{id}: {} vs {v}",
                captured.values[id]
            );
        }
        deep.description.clear();
        let text = captured.to_ron();
        let back = Preset::from_ron(&text).unwrap();
        assert_eq!(back.values, captured.values);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name_for("My Cool  Voice!"), "my_cool_voice.ron");
        assert_eq!(file_name_for("///"), "preset.ron");
    }
}
