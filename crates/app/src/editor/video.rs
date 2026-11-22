//! A slide's video (docs/PLAN.md): Insert → Video, and the Video tab: when it
//! starts, what happens at its end, volume, fit and picture settings. The video plays
//! full screen under the slide's elements while the slide is live.

use std::path::Path;

use doc::{Edit as Op, Fit, SlideVideo, VideoAdjust, VideoEnd, VideoStart};
use egui::{Color32, RichText, Vec2};

use super::text::send_checked;
use crate::ui::{UiAction, UiState};

/// Pick a video file for slide `(g, s)`; a slide that has one keeps its settings.
pub(super) fn pick(
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s): (usize, usize),
) {
    let mut dialog = rfd::FileDialog::new()
        .set_title("Choose a video for this slide")
        .add_filter("Video", crate::media_thumbs::VIDEO_EXTS);
    if let Some(dir) = path.parent() {
        dialog = dialog.set_directory(dir);
    }
    let Some(file) = dialog.pick_file() else {
        return;
    };
    // Relative to the presentation when they share a drive (the library can move as
    // a whole), else the full path.
    let dir = path.parent().unwrap_or(Path::new(""));
    let media = doc::MediaRef {
        path: doc::theme::relative_to(&file, dir),
        sha256: None,
    };
    let video = match d.groups[g].slides[s].video.clone() {
        Some(v) => SlideVideo { media, ..v },
        None => SlideVideo::new(media),
    };
    send_checked(
        st,
        actions,
        path,
        d,
        Op::SetSlideVideo {
            group: g,
            slide: s,
            video: Some(video),
        },
        None,
    );
}

/// The Video tab for slide `(g, s)`.
pub(super) fn video_tab(
    ui: &mut egui::Ui,
    st: &mut UiState,
    actions: &mut Vec<UiAction>,
    path: &Path,
    d: &doc::Document,
    (g, s): (usize, usize),
) {
    let Some(v) = d.groups[g].slides[s].video.clone() else {
        ui.label(RichText::new("This slide has no video.").weak());
        ui.label(
            RichText::new(
                "A video plays full screen under the slide's text while the slide is live, \
                 like a ministry video or a countdown.",
            )
            .small()
            .weak(),
        );
        ui.add_space(4.0);
        if ui.button("Add a video…").clicked() {
            pick(st, actions, path, d, (g, s));
        }
        return;
    };
    let file = v.media.resolve(path);
    let set = |st: &mut UiState, actions: &mut Vec<UiAction>, nv: SlideVideo, key: Option<&str>| {
        send_checked(
            st,
            actions,
            path,
            d,
            Op::SetSlideVideo {
                group: g,
                slide: s,
                video: Some(nv),
            },
            key.map(|k| format!("video-{k}-{g}-{s}")),
        );
    };
    // Poster.
    let w = ui.available_width();
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(w, (w * 9.0 / 16.0).round()), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, Color32::BLACK);
    // With its picture settings, whole (the canvas shows how it fits the slide).
    match st
        .media_thumbs
        .as_mut()
        .and_then(|t| t.get_adjusted(&file, &v.adjust))
    {
        Some(Ok(t)) => {
            crate::media_thumbs::paint_poster(ui.painter(), rect, t, doc::Fit::Fit);
        }
        Some(Err(e)) => {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No preview",
                egui::FontId::proportional(13.0),
                Color32::GRAY,
            );
            let _ = e;
        }
        None => {}
    }
    ui.add_space(4.0);
    ui.label(RichText::new(v.media.file_name()).strong())
        .on_hover_text(file.display().to_string());
    if !file.exists() {
        ui.label(
            RichText::new("The file isn't there (moved, renamed, or on another computer).")
                .small()
                .color(Color32::from_rgb(255, 150, 150)),
        );
    }
    ui.horizontal(|ui| {
        if ui.button("Replace…").clicked() {
            pick(st, actions, path, d, (g, s));
        }
        if ui
            .button(RichText::new("Remove").color(Color32::LIGHT_RED))
            .clicked()
        {
            send_checked(
                st,
                actions,
                path,
                d,
                Op::SetSlideVideo {
                    group: g,
                    slide: s,
                    video: None,
                },
                None,
            );
        }
    });
    ui.separator();

    ui.label(RichText::new("Playback").strong());
    let mut start = v.start;
    ui.radio_value(
        &mut start,
        VideoStart::OnShow,
        "Start when the slide is shown",
    );
    ui.radio_value(
        &mut start,
        VideoStart::OnClick,
        "Wait for Play (in the Preview panel)",
    );
    if start != v.start {
        set(st, actions, SlideVideo { start, ..v.clone() }, None);
    }
    ui.horizontal(|ui| {
        ui.label("At the end");
        let label = |e: VideoEnd| match e {
            VideoEnd::Stop => "the video goes away",
            VideoEnd::Hold => "hold the last frame",
            VideoEnd::Loop => "play it again (loop)",
            VideoEnd::Next => "go to the next slide",
        };
        let mut end = v.end;
        egui::ComboBox::from_id_salt("video-end")
            .selected_text(label(end))
            .show_ui(ui, |ui| {
                for e in [
                    VideoEnd::Stop,
                    VideoEnd::Hold,
                    VideoEnd::Loop,
                    VideoEnd::Next,
                ] {
                    ui.selectable_value(&mut end, e, label(e));
                }
            });
        if end != v.end {
            set(st, actions, SlideVideo { end, ..v.clone() }, None);
        }
    });
    ui.horizontal(|ui| {
        ui.label("Volume");
        let mut pct = (v.volume * 100.0).round() as f32;
        let r = ui.add(
            egui::Slider::new(&mut pct, 0.0..=100.0)
                .suffix("%")
                .integer(),
        );
        if r.changed() {
            let volume = f64::from(pct) / 100.0;
            set(
                st,
                actions,
                SlideVideo {
                    volume,
                    ..v.clone()
                },
                Some("volume"),
            );
        }
    });
    ui.horizontal(|ui| {
        ui.label("Fit");
        let mut fit = v.fit;
        ui.selectable_value(&mut fit, Fit::Fill, "Fill")
            .on_hover_text("Cover the whole screen, cropping the edges if the shapes differ");
        ui.selectable_value(&mut fit, Fit::Fit, "Fit")
            .on_hover_text("The whole picture, black bars if the shapes differ");
        ui.selectable_value(&mut fit, Fit::Stretch, "Stretch");
        if fit != v.fit {
            set(st, actions, SlideVideo { fit, ..v.clone() }, None);
        }
    });
    ui.separator();

    ui.horizontal(|ui| {
        ui.label(RichText::new("Picture").strong());
        if !v.adjust.is_default() && ui.small_button("Reset").clicked() {
            set(
                st,
                actions,
                SlideVideo {
                    adjust: VideoAdjust::default(),
                    ..v.clone()
                },
                None,
            );
        }
    });
    let mut adj = v.adjust;
    let mut changed = None;
    egui::Grid::new("video-adjust")
        .num_columns(2)
        .spacing(Vec2::new(8.0, 4.0))
        .show(ui, |ui| {
            for (name, key, value) in [
                ("Brightness", "brightness", &mut adj.brightness),
                ("Contrast", "contrast", &mut adj.contrast),
                ("Saturation", "saturation", &mut adj.saturation),
                ("Gamma", "gamma", &mut adj.gamma),
                ("Hue", "hue", &mut adj.hue),
            ] {
                ui.label(name);
                let mut x = (*value * 100.0).round() as f32;
                if ui
                    .add(egui::Slider::new(&mut x, -100.0..=100.0).integer())
                    .changed()
                {
                    *value = f64::from(x) / 100.0;
                    changed = Some(key);
                }
                ui.end_row();
            }
        });
    if let Some(key) = changed {
        set(
            st,
            actions,
            SlideVideo {
                adjust: adj,
                ..v.clone()
            },
            Some(key),
        );
    }
    ui.add_space(4.0);
    ui.label(
        RichText::new("Changes show on the output the next time the slide goes live.")
            .small()
            .weak(),
    );
}
