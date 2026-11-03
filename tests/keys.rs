//! Editor keyboard shortcuts, driven through headless egui frames.

use glide::app::editor::{Editor, Selection};
use glide::app::home::{self, Devices, Settings, Thumbs};
use glide::render::Gpu;

#[test]
fn cmd_x_deletes_the_selection() {
    let dir = std::env::temp_dir().join(format!("glide-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Keys.glide");
    glide::synth::write_package(&path, 3.0, 1280, 800).unwrap();

    let gpu = Gpu::headless().unwrap();
    let mut renderer = egui_wgpu::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm, egui_wgpu::RendererOptions::default());
    let ctx = egui::Context::default();
    glide::app::theme::apply(&ctx);
    let mut ed = Editor::open(&path, gpu).unwrap();
    let (mut settings, mut devices, mut thumbs) = (Settings { projects_dir: Some(dir.clone()), ..Settings::default() }, Devices::default(), Thumbs::default());
    let projects = home::scan_projects(&dir);
    let mut frame = |ed: &mut Editor, t: f64, events: Vec<egui::Event>| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1320.0, 840.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let mut chrome = home::Chrome { settings: &mut settings, devices: &mut devices, projects: &projects, thumbs: &mut thumbs, action: home::Action::None };
            ed.ui(ui, &mut renderer, &mut chrome);
        });
        // No window to upload textures to.
        out.textures_delta.clear();
    };

    frame(&mut ed, 0.0, vec![]);
    let before = ed.project.timeline.zooms.len();
    ed.add_zoom();
    let id = ed.project.timeline.zooms.last().unwrap().id;
    ed.selection = Selection::Zoom(id);
    assert_eq!(ed.project.timeline.zooms.len(), before + 1);

    // egui reports ⌘X as a Cut event, not as a key press.
    frame(&mut ed, 0.016, vec![egui::Event::Cut]);
    assert_eq!(ed.project.timeline.zooms.len(), before);
    assert!(ed.project.timeline.zooms.iter().all(|z| z.id != id));
    assert_eq!(ed.selection, Selection::None);

    let _ = std::fs::remove_dir_all(&dir);
}
