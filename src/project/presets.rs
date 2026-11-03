//! Templates: a project's settings saved by name to
//! `~/Library/Application Support/Glide/presets/<name>.json`, applied to any
//! project, and optionally used for every new recording. A template holds
//! the style (background, aspect ratio, captions, camera, audio and music),
//! the export settings and the effects that cover the whole recording, and
//! keeps its own copies of the background image and the music, so it still
//! works after the originals move. Templates saved before they held more
//! than the style still load.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::model::{Background, Effect, ExportSettings, Project, Style};
use super::{Package, read_json, write_json};

pub fn dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join("Library/Application Support/Glide/presets")
}

fn default_marker(dir: &Path) -> PathBuf {
    dir.join(".default")
}

/// An effect this close to both ends of the recording covers all of it.
const WHOLE_MS: f64 = 100.0;

/// What a template file holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Template {
    pub style: Style,
    /// None in templates saved before export settings were kept.
    pub export: Option<ExportSettings>,
    /// Effects over the whole recording; their times don't matter.
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub path: PathBuf,
}

/// Templates in `dir`, sorted by name.
pub fn list_in(dir: &Path) -> Vec<Preset> {
    let mut out: Vec<Preset> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "json" {
                return None;
            }
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some(Preset { name, path })
        })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

pub fn list() -> Vec<Preset> {
    list_in(&dir())
}

fn sanitize(name: &str) -> Result<String> {
    let n: String = name.trim().chars().map(|c| if matches!(c, '/' | ':' | '\\') { '-' } else { c }).collect();
    if n.is_empty() || n.starts_with('.') {
        bail!("template names can't be empty or start with a dot");
    }
    Ok(n)
}

/// Whether an effect covers a whole recording `duration` ms long.
fn covers_whole(e: &Effect, duration: f64) -> bool {
    e.start <= WHOLE_MS && e.end >= duration - WHOLE_MS
}

/// Copy `src` (relative paths are inside `package_dir`) into the template
/// folder's assets as `<name>-<kind>.<ext>`, and return the copy's path.
/// None when there is nothing to copy; a file already in `dir` stays put.
fn keep_copy(dir: &Path, name: &str, kind: &str, src: &str, package_dir: Option<&Path>) -> Result<Option<String>> {
    let src = match (Path::new(src).is_relative(), package_dir) {
        (true, Some(pkg)) => pkg.join(src),
        _ => PathBuf::from(src),
    };
    if !src.exists() || src.starts_with(dir) {
        return Ok(None);
    }
    let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("bin").to_string();
    let dst = dir.join("assets").join(format!("{name}-{kind}.{ext}"));
    std::fs::create_dir_all(dst.parent().unwrap())?;
    std::fs::copy(&src, &dst)?;
    Ok(Some(dst.to_string_lossy().into_owned()))
}

/// Save `project`'s settings as a template: its style, export settings and
/// the effects over the whole recording (`duration` ms). The background
/// image and the music are copied next to the template.
pub fn save_in(dir: &Path, name: &str, project: &Project, duration: f64, package_dir: Option<&Path>) -> Result<Preset> {
    let name = sanitize(name)?;
    std::fs::create_dir_all(dir)?;
    let mut style = project.style.clone();
    if let Background::Image { path } = &mut style.background
        && let Some(copy) = keep_copy(dir, &name, "background", path, package_dir)?
    {
        *path = copy;
    }
    if let Some(music) = &mut style.audio.music
        && let Some(copy) = keep_copy(dir, &name, "music", music, package_dir)?
    {
        *music = copy;
    }
    let effects = project.timeline.effects.iter().filter(|e| covers_whole(e, duration)).copied().collect();
    let template = Template { style, export: Some(project.export), effects };
    let path = dir.join(format!("{name}.json"));
    write_json(&path, &template)?;
    Ok(Preset { name, path })
}

pub fn save(name: &str, project: &Project, duration: f64, package_dir: Option<&Path>) -> Result<Preset> {
    save_in(&dir(), name, project, duration, package_dir)
}

/// A template file: the current kind, or an older one that held only a style.
fn read_template(path: &Path) -> Result<Template> {
    let v: serde_json::Value = read_json(path)?;
    if v.get("style").is_some() {
        Ok(serde_json::from_value(v)?)
    } else {
        Ok(Template { style: serde_json::from_value(v)?, ..Default::default() })
    }
}

pub fn load(p: &Preset) -> Result<Template> {
    read_template(&p.path)
}

pub fn delete(p: &Preset) -> Result<()> {
    if default_name_in(p.path.parent().unwrap_or(Path::new("."))).as_deref() == Some(p.name.as_str()) {
        let _ = std::fs::remove_file(default_marker(p.path.parent().unwrap()));
    }
    std::fs::remove_file(&p.path)?;
    Ok(())
}

pub fn default_name_in(dir: &Path) -> Option<String> {
    let n = std::fs::read_to_string(default_marker(dir)).ok()?;
    let n = n.trim().to_string();
    dir.join(format!("{n}.json")).exists().then_some(n)
}

/// Mark a template as the one for new recordings (None clears it).
pub fn set_default_in(dir: &Path, name: Option<&str>) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    match name {
        Some(n) => std::fs::write(default_marker(dir), n)?,
        None => {
            let _ = std::fs::remove_file(default_marker(dir));
        }
    }
    Ok(())
}

pub fn default_name() -> Option<String> {
    default_name_in(&dir())
}

pub fn set_default(name: Option<&str>) -> Result<()> {
    set_default_in(&dir(), name)
}

/// The template for new recordings, if one is set.
pub fn default_template() -> Option<Template> {
    let d = dir();
    let name = default_name_in(&d)?;
    read_template(&d.join(format!("{name}.json"))).ok()
}

/// Apply a style, keeping what belongs to the recording rather than the
/// look: crop and camera position.
pub fn apply(style: &mut Style, preset: &Style) {
    let crop = style.crop;
    let camera_position = style.camera.position;
    *style = preset.clone();
    style.crop = crop;
    style.camera.position = camera_position;
}

/// Apply a template to a project for a recording `duration` ms long: the
/// style (see `apply`), the export settings it has, and its effects over the
/// whole recording, replacing the project's own whole-recording effects so
/// applying it twice doesn't double them. With a `package`, the music is
/// copied into it, so the project doesn't depend on the template's copy.
pub fn apply_template(project: &mut Project, t: &Template, duration: f64, package: Option<&Package>) {
    apply(&mut project.style, &t.style);
    if let Some(export) = t.export {
        project.export = export;
    }
    project.timeline.effects.retain(|e| !covers_whole(e, duration));
    for e in &t.effects {
        let id = project.timeline.new_id();
        project.timeline.effects.push(Effect { id, start: 0.0, end: duration, ..*e });
    }
    if let (Some(pkg), Some(music)) = (package, project.style.audio.music.clone())
        && !Path::new(&music).starts_with(&pkg.root)
        && let Ok(rel) = pkg.import_asset(Path::new(&music), "music")
    {
        project.style.audio.music = Some(pkg.root.join(rel).to_string_lossy().into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::model::{EffectKind, ExportFormat};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glide-templates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn save_list_default_and_delete() {
        let dir = temp("list");
        let mut project = Project::default();
        project.style.padding = 12.5;
        let p = save_in(&dir, "Launch video", &project, 10_000.0, None).unwrap();
        save_in(&dir, "a/b", &Project::default(), 10_000.0, None).unwrap();
        let names: Vec<_> = list_in(&dir).into_iter().map(|p| p.name).collect();
        assert_eq!(names, vec!["a-b", "Launch video"]);
        assert_eq!(load(&p).unwrap().style.padding, 12.5);
        set_default_in(&dir, Some("Launch video")).unwrap();
        assert_eq!(default_name_in(&dir).as_deref(), Some("Launch video"));
        delete(&p).unwrap();
        assert_eq!(default_name_in(&dir), None);
        assert!(save_in(&dir, "  ", &project, 10_000.0, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_template_keeps_export_settings_and_whole_recording_effects() {
        let dir = temp("keeps");
        let mut project = Project::default();
        project.export = ExportSettings::tiktok();
        project.timeline.effects.push(Effect { id: 1, start: 0.0, end: 10_000.0, kind: EffectKind::Pulse, strength: 0.8, ..Default::default() });
        project.timeline.effects.push(Effect { id: 2, start: 2000.0, end: 3000.0, ..Default::default() });
        let p = save_in(&dir, "TikTok", &project, 10_000.0, None).unwrap();
        let t = load(&p).unwrap();
        assert_eq!(t.export, Some(ExportSettings::tiktok()));
        assert_eq!(t.effects.len(), 1, "only the effect over the whole recording");
        assert_eq!((t.effects[0].kind, t.effects[0].strength), (EffectKind::Pulse, 0.8));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_older_style_only_preset_still_loads() {
        let dir = temp("old");
        let mut style = Style::default();
        style.padding = 21.0;
        write_json(&dir.join("Old.json"), &style).unwrap();
        let t = load(&list_in(&dir)[0]).unwrap();
        assert_eq!(t.style.padding, 21.0);
        assert_eq!((t.export, t.effects.len()), (None, 0));
        // Applying it leaves the export settings alone.
        let mut project = Project::default();
        project.export.format = ExportFormat::Gif;
        apply_template(&mut project, &t, 5000.0, None);
        assert_eq!(project.export.format, ExportFormat::Gif);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_music_is_copied_so_the_template_outlives_the_original() {
        let dir = temp("music");
        let original = dir.join("song.mp3");
        std::fs::write(&original, b"not really music").unwrap();
        let mut project = Project::default();
        project.style.audio.music = Some(original.to_string_lossy().into_owned());
        let presets = dir.join("presets");
        let p = save_in(&presets, "With music", &project, 10_000.0, None).unwrap();
        std::fs::remove_file(&original).unwrap();
        let kept = load(&p).unwrap().style.audio.music.unwrap();
        assert!(kept.starts_with(&*presets.to_string_lossy()), "{kept}");
        assert_eq!(std::fs::read(&kept).unwrap(), b"not really music");
        // Applied with a package, the project gets its own copy.
        let pkg = Package::new(dir.join("Demo.glide"));
        let mut target = Project::default();
        apply_template(&mut target, &load(&p).unwrap(), 5000.0, Some(&pkg));
        let in_package = target.style.audio.music.unwrap();
        assert!(in_package.starts_with(&*pkg.root.to_string_lossy()), "{in_package}");
        assert_eq!(std::fs::read(&in_package).unwrap(), b"not really music");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn applying_fits_the_effects_to_the_recording_and_never_doubles_them() {
        let t = Template {
            style: Style { padding: 30.0, ..Default::default() },
            export: Some(ExportSettings::tiktok()),
            effects: vec![Effect { id: 77, start: 0.0, end: 99_000.0, kind: EffectKind::Pulse, ..Default::default() }],
        };
        let mut project = Project::default();
        project.style.crop.x = 0.1;
        project.style.camera.position.x = 0.2;
        project.timeline.effects.push(Effect { id: 5, start: 1000.0, end: 2000.0, ..Default::default() });
        apply_template(&mut project, &t, 6000.0, None);
        apply_template(&mut project, &t, 6000.0, None);
        assert_eq!(project.style.padding, 30.0);
        assert_eq!((project.style.crop.x, project.style.camera.position.x), (0.1, 0.2));
        assert_eq!(project.export, ExportSettings::tiktok());
        let whole: Vec<_> = project.timeline.effects.iter().filter(|e| e.start == 0.0 && e.end == 6000.0).collect();
        assert_eq!(whole.len(), 1, "applied twice, still one");
        assert_eq!(whole[0].kind, EffectKind::Pulse);
        assert_ne!(whole[0].id, 77, "a fresh id in this project");
        assert!(project.timeline.effects.iter().any(|e| e.id == 5), "a shorter effect of the project's own stays");
    }
}
