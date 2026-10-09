//! The voice-model library: list/delete installed voices, fetch base
//! models, and import a voice from a link (Hugging Face page or a direct
//! `.pth` / `.onnx` / `.zip` URL), converting `.pth` with the embedded
//! exporter in a Python venv. All long work runs on a background thread
//! and reports through [`ImportStatus`].

use anyhow::{Context as _, Result, anyhow, bail};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The RVC exporter script, shipped inside the binary.
const EXPORTER_PY: &str = include_str!("../../../../scripts/rvc_export.py");
const HF_BASE_REPO: &str = "TigreGotico/voiceclonnx-rvc";

#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Idle,
    Downloading {
        file: String,
        done: u64,
        total: Option<u64>,
    },
    Extracting,
    PreparingConverter,
    Converting,
    Done(String),
    Failed(String),
}

#[derive(Default)]
pub struct ImportStatus {
    stage: Mutex<Option<Stage>>,
    cancel: AtomicBool,
    pub log: Mutex<Vec<String>>,
}

impl ImportStatus {
    pub fn stage(&self) -> Stage {
        self.stage
            .lock()
            .ok()
            .and_then(|s| s.clone())
            .unwrap_or(Stage::Idle)
    }
    fn set(&self, stage: Stage) {
        if let Ok(mut s) = self.stage.lock() {
            *s = Some(stage);
        }
    }
    fn note(&self, line: impl Into<String>) {
        let line = line.into();
        log::info!("import: {line}");
        if let Ok(mut l) = self.log.lock() {
            l.push(line);
            if l.len() > 200 {
                l.remove(0);
            }
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn is_finished(&self) -> bool {
        matches!(
            self.stage(),
            Stage::Done(_) | Stage::Failed(_) | Stage::Idle
        )
    }
    fn check_cancel(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceInfo {
    pub name: String,
    pub path: PathBuf,
    pub size_mb: f32,
    pub sample_rate: Option<u32>,
    pub source: Option<String>,
    pub has_index: bool,
    /// Speakers the model was trained with (`ds` input); 1 for most voices.
    pub speakers: u32,
    /// Speaker the voice starts with when selected.
    pub default_speaker: u32,
    /// Optional names per speaker ("Male", "Female"…), may be shorter than `speakers`.
    pub speaker_names: Vec<String>,
}

impl VoiceInfo {
    pub fn speaker_label(&self, idx: u32) -> String {
        self.speaker_names
            .get(idx as usize)
            .filter(|n| !n.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| format!("Speaker {}", idx + 1))
    }
}

fn read_sidecar(path: &Path) -> serde_json::Value {
    std::fs::read_to_string(path.with_extension("json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn write_sidecar(path: &Path, meta: &serde_json::Value) -> Result<()> {
    std::fs::write(path.with_extension("json"), meta.to_string())
        .with_context(|| format!("writing {}", path.with_extension("json").display()))
}

/// Speaker count from the sidecar (user-editable), probing the model once
/// if unknown or implausible.
fn speakers_of(path: &Path, meta: &mut serde_json::Value) -> u32 {
    if let Some(n) = meta["speakers"].as_u64()
        && (1..=super::onnx_meta::MAX_SPEAKERS as u64).contains(&n)
    {
        return n as u32;
    }
    let n = super::onnx_meta::speaker_count(path);
    meta["speakers"] = serde_json::json!(n);
    if let Err(e) = write_sidecar(path, meta) {
        log::debug!("{e:#}");
    }
    n
}

/// Which speaker a voice should start with (its pinned default).
pub fn default_speaker_for(path: &Path) -> i32 {
    read_sidecar(path)["default_speaker"].as_u64().unwrap_or(0) as i32
}

/// Speaker count for the loaded voice (used by the worker to clamp `ds`).
pub fn speaker_count_for(path: &Path) -> u32 {
    let mut meta = read_sidecar(path);
    speakers_of(path, &mut meta)
}

/// Tell the app how many speakers a model really has (RVC files do not say).
pub fn set_voice_speakers(path: &Path, speakers: u32) -> Result<()> {
    let mut meta = read_sidecar(path);
    let n = speakers.clamp(1, super::onnx_meta::MAX_SPEAKERS);
    meta["speakers"] = serde_json::json!(n);
    if meta["default_speaker"].as_u64().unwrap_or(0) >= n as u64 {
        meta["default_speaker"] = serde_json::json!(0);
    }
    write_sidecar(path, &meta)
}

pub fn set_voice_default_speaker(path: &Path, speaker: u32) -> Result<()> {
    let mut meta = read_sidecar(path);
    meta["default_speaker"] = serde_json::json!(speaker);
    write_sidecar(path, &meta)
}

pub fn set_voice_speaker_name(path: &Path, speaker: u32, name: &str) -> Result<()> {
    let mut meta = read_sidecar(path);
    let mut names: Vec<String> = meta["speaker_names"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default();
    while names.len() <= speaker as usize {
        names.push(String::new());
    }
    names[speaker as usize] = name.trim().to_string();
    meta["speaker_names"] = serde_json::json!(names);
    write_sidecar(path, &meta)
}

/// Installed voices with size and sidecar metadata.
pub fn voices() -> Vec<VoiceInfo> {
    super::list_voices()
        .into_iter()
        .map(|v| {
            let size_mb = std::fs::metadata(&v.path)
                .map(|m| m.len() as f32 / 1e6)
                .unwrap_or(0.0);
            let mut meta = read_sidecar(&v.path);
            let speakers = speakers_of(&v.path, &mut meta);
            VoiceInfo {
                name: v.name,
                has_index: v.path.with_extension("index").is_file(),
                size_mb,
                sample_rate: meta["sample_rate"].as_u64().map(|r| r as u32),
                source: meta["source"].as_str().map(str::to_string),
                speakers,
                default_speaker: (meta["default_speaker"].as_u64().unwrap_or(0) as u32)
                    .min(speakers - 1),
                speaker_names: meta["speaker_names"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|x| x.as_str().unwrap_or("").to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                path: v.path,
            }
        })
        .collect()
}

/// Remove a voice and its sidecar.
pub fn delete_voice(path: &Path) -> Result<()> {
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    let _ = std::fs::remove_file(path.with_extension("json"));
    let _ = std::fs::remove_file(path.with_extension("index"));
    Ok(())
}

pub fn base_models_present() -> bool {
    super::models_dir().is_some_and(|d| super::find_base_models(&d).is_some())
}

/// Start a background import. Returns immediately; poll the status.
pub fn start_import(url: String, name: Option<String>) -> Arc<ImportStatus> {
    let status = Arc::new(ImportStatus::default());
    status.set(Stage::Downloading {
        file: "…".into(),
        done: 0,
        total: None,
    });
    let st = status.clone();
    std::thread::Builder::new()
        .name("voice-import".into())
        .spawn(move || match import(&url, name.as_deref(), &st) {
            Ok(name) => st.set(Stage::Done(name)),
            Err(e) => {
                st.note(format!("failed: {e:#}"));
                st.set(Stage::Failed(format!("{e:#}")));
            }
        })
        .expect("spawn import thread");
    status
}

/// Start fetching the base models (ContentVec + RMVPE) in the background.
pub fn start_base_models_download() -> Arc<ImportStatus> {
    let status = Arc::new(ImportStatus::default());
    status.set(Stage::Downloading {
        file: "base models".into(),
        done: 0,
        total: None,
    });
    let st = status.clone();
    std::thread::Builder::new()
        .name("base-models".into())
        .spawn(move || {
            let run = || -> Result<()> {
                let dir = super::models_dir().ok_or_else(|| anyhow!("no data directory"))?;
                std::fs::create_dir_all(&dir)?;
                for f in ["contentvec_768l12.onnx", "rmvpe.onnx"] {
                    let dest = dir.join(f);
                    if dest.is_file() {
                        continue;
                    }
                    let url = format!("https://huggingface.co/{HF_BASE_REPO}/resolve/main/{f}");
                    download(&url, &dest, &st)?;
                }
                Ok(())
            };
            match run() {
                Ok(()) => st.set(Stage::Done("base models".into())),
                Err(e) => st.set(Stage::Failed(format!("{e:#}"))),
            }
        })
        .expect("spawn download thread");
    status
}

fn sanitize(name: &str) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    let s = s
        .trim_end_matches(".onnx")
        .trim_end_matches(".pth")
        .trim_matches('_');
    if s.is_empty() {
        "voice".into()
    } else {
        s.to_string()
    }
}

/// What a pasted link points at.
enum Source {
    File {
        url: String,
        file_name: String,
        hint: String,
    },
    HfRepo {
        repo: String,
    },
}

fn classify(url: &str) -> Result<Source> {
    let url = url.trim().trim_end_matches('/');
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    let lower = no_query.to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix("https://huggingface.co/")
        .or_else(|| lower.strip_prefix("http://huggingface.co/"))
        .or_else(|| lower.strip_prefix("https://hf.co/"))
    {
        let parts: Vec<&str> = no_query[no_query.len() - rest.len()..].split('/').collect();
        if parts.len() < 2 {
            bail!("that Hugging Face link has no user/repo");
        }
        let repo = format!("{}/{}", parts[0], parts[1]);
        if parts.len() >= 4 && (parts[2] == "blob" || parts[2] == "resolve") {
            let file = parts[4..].join("/");
            let file_name = file.rsplit('/').next().unwrap_or(&file).to_string();
            return Ok(Source::File {
                url: format!("https://huggingface.co/{repo}/resolve/{}/{file}", parts[3]),
                hint: parts[1].to_string(),
                file_name,
            });
        }
        return Ok(Source::HfRepo { repo });
    }
    let file_name = no_query
        .rsplit('/')
        .next()
        .unwrap_or("download")
        .to_string();
    Ok(Source::File {
        url: url.to_string(),
        hint: file_name
            .rsplit('.')
            .next_back()
            .map(|_| file_name.clone())
            .unwrap_or_default(),
        file_name,
    })
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(20))
        .timeout_read(std::time::Duration::from_secs(120))
        .user_agent("voice-changer/0.1")
        .build()
}

/// Pick the best file from a Hugging Face repo listing: a ready `.onnx`,
/// else a voice `.pth` (not the D_/G_ training checkpoints), else a `.zip`.
/// Also returns the URL of a `.index` retrieval file when the repo has one.
fn pick_hf_file(repo: &str, status: &ImportStatus) -> Result<(String, String, Option<String>)> {
    let api = format!("https://huggingface.co/api/models/{repo}");
    let body = agent()
        .get(&api)
        .call()
        .with_context(|| format!("listing {repo}"))?
        .into_string()?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let files: Vec<String> = v["siblings"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s["rfilename"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    status.note(format!("{repo}: {} files", files.len()));
    let lower = |f: &String| f.to_ascii_lowercase();
    let is_ckpt = |f: &String| {
        let base = f.rsplit('/').next().unwrap_or(f).to_ascii_lowercase();
        base.starts_with("d_") || base.starts_with("g_")
    };
    let pick = files
        .iter()
        .find(|f| {
            lower(f).ends_with(".onnx")
                && !lower(f).contains("rmvpe")
                && !lower(f).contains("hubert")
                && !lower(f).contains("contentvec")
                && !lower(f).contains("vec-")
        })
        .or_else(|| {
            files
                .iter()
                .find(|f| lower(f).ends_with(".pth") && !is_ckpt(f))
        })
        .or_else(|| files.iter().find(|f| lower(f).ends_with(".zip")))
        .ok_or_else(|| anyhow!("no .onnx, .pth or .zip voice model found in {repo}"))?;
    let index = files
        .iter()
        .find(|f| lower(f).ends_with(".index"))
        .map(|f| format!("https://huggingface.co/{repo}/resolve/main/{f}"));
    Ok((
        format!("https://huggingface.co/{repo}/resolve/main/{pick}"),
        pick.rsplit('/').next().unwrap_or(pick).to_string(),
        index,
    ))
}

fn download(url: &str, dest: &Path, status: &ImportStatus) -> Result<()> {
    let file_name = dest
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    status.note(format!("downloading {url}"));
    let resp = agent()
        .get(url)
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok());
    let mut reader = resp.into_reader();
    let tmp = dest.with_extension("part");
    let mut out = std::fs::File::create(&tmp)?;
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        status.check_cancel().inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        status.set(Stage::Downloading {
            file: file_name.clone(),
            done,
            total,
        });
    }
    out.flush()?;
    drop(out);
    if done < 1000 {
        let _ = std::fs::remove_file(&tmp);
        bail!("download was only {done} bytes; the link probably isn't a model file");
    }
    std::fs::rename(&tmp, dest)?;
    status.note(format!("got {} ({:.1} MB)", file_name, done as f32 / 1e6));
    Ok(())
}

fn tools_dir() -> Result<PathBuf> {
    let d = super::models_dir()
        .and_then(|m| m.parent().map(|p| p.join("tools")))
        .ok_or_else(|| anyhow!("no data directory"))?;
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn run_logged(status: &ImportStatus, cmd: &mut Command) -> Result<()> {
    let out = cmd
        .output()
        .with_context(|| format!("running {:?}", cmd.get_program()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    for line in text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.contains("Warning"))
    {
        status.note(line.to_string());
    }
    if !out.status.success() {
        let tail: Vec<&str> = text
            .lines()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        bail!("{:?} failed:\n{}", cmd.get_program(), tail.join("\n"));
    }
    Ok(())
}

/// Create the Python environment with torch (CPU) and the exporter.
fn ensure_converter(status: &ImportStatus) -> Result<PathBuf> {
    let tools = tools_dir()?;
    let venv = tools.join("venv");
    let python = venv.join("bin/python");
    let script = tools.join("rvc_export.py");
    std::fs::write(&script, EXPORTER_PY)?;
    if !python.is_file() {
        status.set(Stage::PreparingConverter);
        status.note("creating Python environment for the converter (one-time)");
        run_logged(
            status,
            Command::new("python3").args(["-m", "venv"]).arg(&venv),
        )?;
    }
    let probe = Command::new(&python)
        .args(["-c", "import torch, onnx, numpy, onnxscript, scipy"])
        .output();
    if !probe.map(|o| o.status.success()).unwrap_or(false) {
        status.set(Stage::PreparingConverter);
        status.note("installing torch (CPU build, ~300 MB) and ONNX tools");
        let pip = venv.join("bin/pip");
        run_logged(
            status,
            Command::new(&pip).args(["install", "--quiet", "--upgrade", "pip"]),
        )?;
        run_logged(
            status,
            Command::new(&pip).args([
                "install",
                "--quiet",
                "torch",
                "--index-url",
                "https://download.pytorch.org/whl/cpu",
            ]),
        )?;
        run_logged(
            status,
            Command::new(&pip).args(["install", "--quiet", "onnx", "numpy", "scipy", "onnxscript"]),
        )?;
    }
    Ok(python)
}

fn convert_pth(pth: &Path, out: &Path, status: &ImportStatus) -> Result<()> {
    let python = ensure_converter(status)?;
    status.set(Stage::Converting);
    status.note(format!(
        "converting {} (about a minute)",
        pth.file_name().unwrap_or_default().to_string_lossy()
    ));
    let tools = tools_dir()?;
    run_logged(
        status,
        Command::new(&python)
            .env("VC_TOOLS", &tools)
            .arg(tools.join("rvc_export.py"))
            .arg(pth)
            .arg(out),
    )
}

fn extract_zip(zip_path: &Path, status: &ImportStatus) -> Result<PathBuf> {
    status.set(Stage::Extracting);
    let dir = zip_path.with_extension("unzipped");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut best: Option<(u8, PathBuf)> = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let name = enclosed
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        let rank = if lower.ends_with(".onnx") {
            3
        } else if lower.ends_with(".pth") && !lower.starts_with("d_") && !lower.starts_with("g_") {
            2
        } else if lower.ends_with(".index") {
            1
        } else {
            0
        };
        if rank == 0 || entry.is_dir() {
            continue;
        }
        let dest = dir.join(&name);
        let mut out = std::fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut out)?;
        status.note(format!("extracted {name}"));
        if rank > 1 && best.as_ref().is_none_or(|(r, _)| rank > *r) {
            best = Some((rank, dest));
        }
    }
    best.map(|(_, p)| p)
        .ok_or_else(|| anyhow!("the zip contains no .pth or .onnx model"))
}

fn import(url: &str, name: Option<&str>, status: &ImportStatus) -> Result<String> {
    let voices_dir = super::voices_dir().ok_or_else(|| anyhow!("no data directory"))?;
    std::fs::create_dir_all(&voices_dir)?;
    let downloads = tools_dir()?.join("downloads");
    std::fs::create_dir_all(&downloads)?;

    let (file_url, file_name, hint, index_url) = match classify(url)? {
        Source::File {
            url,
            file_name,
            hint,
        } => (url, file_name, hint, None),
        Source::HfRepo { repo } => {
            let (u, f, idx) = pick_hf_file(&repo, status)?;
            let repo_name = repo.split('/').nth(1).unwrap_or(&repo).to_string();
            (u, f, repo_name, idx)
        }
    };
    let display = sanitize(name.filter(|n| !n.trim().is_empty()).unwrap_or(&hint));
    let target = voices_dir.join(format!("{display}.onnx"));
    if target.exists() {
        bail!("a voice named {display:?} already exists; delete it first or give a different name");
    }

    let downloaded = downloads.join(&file_name);
    download(&file_url, &downloaded, status)?;
    status.check_cancel()?;

    let lower = file_name.to_ascii_lowercase();
    let model_file = if lower.ends_with(".zip") {
        extract_zip(&downloaded, status)?
    } else {
        downloaded.clone()
    };
    let model_lower = model_file.to_string_lossy().to_ascii_lowercase();

    if model_lower.ends_with(".onnx") {
        std::fs::rename(&model_file, &target)
            .or_else(|_| std::fs::copy(&model_file, &target).map(|_| ()))?;
        let meta = serde_json::json!({ "source": url });
        let _ = std::fs::write(target.with_extension("json"), meta.to_string());
    } else if model_lower.ends_with(".pth") {
        convert_pth(&model_file, &target, status)?;
        // The exporter writes a sidecar; add where it came from.
        let sidecar = target.with_extension("json");
        let mut meta: serde_json::Value = std::fs::read_to_string(&sidecar)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        meta["source"] = serde_json::Value::String(url.to_string());
        let _ = std::fs::write(&sidecar, meta.to_string());
    } else {
        bail!("{} is not a .pth or .onnx model", model_file.display());
    }
    // Retrieval index: from the repo, or extracted from the zip.
    let index_target = target.with_extension("index");
    if let Some(iu) = index_url {
        if let Err(e) = download(&iu, &index_target, status) {
            status.note(format!("no retrieval index: {e}"));
            let _ = std::fs::remove_file(&index_target);
        }
    } else if lower.ends_with(".zip") {
        let dir = downloaded.with_extension("unzipped");
        if let Ok(read) = std::fs::read_dir(&dir) {
            for e in read.flatten() {
                if e.path().extension().is_some_and(|x| x == "index") {
                    let _ = std::fs::rename(e.path(), &index_target)
                        .or_else(|_| std::fs::copy(e.path(), &index_target).map(|_| ()));
                    break;
                }
            }
        }
    }
    if index_target.is_file() {
        match super::index::RetrievalIndex::load(&index_target) {
            Ok(idx) => status.note(format!("retrieval index: {} vectors", idx.ntotal)),
            Err(e) => {
                status.note(format!("index unusable ({e}); removed"));
                let _ = std::fs::remove_file(&index_target);
            }
        }
    }
    // Tidy the download cache (keep nothing big around).
    let _ = std::fs::remove_file(&downloaded);
    let _ = std::fs::remove_dir_all(downloaded.with_extension("unzipped"));
    status.note(format!("installed {display}"));
    Ok(display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_links() {
        match classify("https://huggingface.co/user/My_Voice").unwrap() {
            Source::HfRepo { repo } => assert_eq!(repo, "user/My_Voice"),
            _ => panic!(),
        }
        match classify("https://huggingface.co/user/repo/blob/main/sub/model.pth?download=true")
            .unwrap()
        {
            Source::File { url, file_name, .. } => {
                assert_eq!(
                    url,
                    "https://huggingface.co/user/repo/resolve/main/sub/model.pth"
                );
                assert_eq!(file_name, "model.pth");
            }
            _ => panic!(),
        }
        match classify("https://example.com/dl/voice.zip").unwrap() {
            Source::File { file_name, .. } => assert_eq!(file_name, "voice.zip"),
            _ => panic!(),
        }
        assert_eq!(
            sanitize(" Kobo Kanaeru (RVC v2).pth "),
            "Kobo_Kanaeru_RVC_v2"
        );
    }
}
