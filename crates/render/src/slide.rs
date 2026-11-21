//! Lay out and rasterise one slide.
//!
//! Assumptions (docs/PLAN.md Unknowns): one RTF point = one canvas pixel (the
//! native run size is in canvas pixels); RTF "auto" colour (no `\cf`) = black. `verticalAlignment`: 0 = middle is **verified** (every real
//! file uses 0 and PP6 shows it centred); 1 = top, 2 = bottom are assumed.
//! Layout happens directly at output scale (font sizes and boxes multiplied by
//! the fit scale), so text stays sharp on any output resolution.

use std::time::Instant;

use cosmic_text::Color;
use doc::{BackgroundImage, Document, Element, Fit, ImageBox, Rgba, Slide, TextBox, TextProblem};

use crate::effects;
use crate::error::{RenderError, RenderWarning};
use crate::fonts::Fonts;
use crate::frame::{Placement, SlideFrame, mul};
use crate::picture;
use crate::text::{self, Ctx, TextLayout};

/// Render `slide` of `doc` for an output of `width × height` pixels.
pub fn render_slide(
    fonts: &mut Fonts,
    doc: &Document,
    slide: &Slide,
    width: u32,
    height: u32,
) -> Result<SlideFrame, RenderError> {
    let _span = tracing::debug_span!("render_slide", slide = %slide.uuid, width, height).entered();
    let started = Instant::now();
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(RenderError::BadTarget { width, height }.logged());
    }
    let placement = Placement::fit(doc.canvas, width, height);
    let mut frame = SlideFrame {
        width,
        height,
        placement,
        // A slide's video plays under the elements: no colour may cover it.
        background: if slide.video.is_some() {
            None
        } else {
            slide.background.or(doc.background).map(to_rgba8)
        },
        text: vec![0; width as usize * height as usize * 4],
        warnings: Vec::new(),
        render_time: Default::default(),
    };
    if let Some(bg) = &slide.background_image {
        render_background_image(doc, bg, &mut frame);
    }
    for el in slide.elements.iter().filter(|e| !e.hidden()) {
        match el {
            Element::Text(t) => render_text(fonts, doc, slide, t, &mut frame)?,
            Element::Image(i) => render_image(doc, i, &mut frame),
            Element::Pro6(raw) => frame.warnings.push(RenderWarning {
                element_uuid: raw.uuid.clone(),
                message: format!("<{}> elements aren't drawn yet", raw.kind),
                overflow: false,
                media: false,
            }),
        }
    }
    frame.render_time = started.elapsed();
    tracing::debug!(
        elapsed_us = frame.render_time.as_micros() as u64,
        warnings = frame.warnings.len(),
        "rendered"
    );
    Ok(frame)
}

fn to_rgba8(c: Rgba) -> [u8; 4] {
    c.to_rgba8()
}

fn warning(el: &TextBox, message: String) -> RenderWarning {
    RenderWarning {
        element_uuid: el.uuid.clone(),
        message,
        overflow: false,
        media: false,
    }
}

/// Straight-alpha colour with `opacity` applied.
fn with_opacity(c: Rgba, opacity: f64) -> [u8; 4] {
    let [r, g, b, a] = c.to_rgba8();
    [
        r,
        g,
        b,
        (f64::from(a) * opacity.clamp(0.0, 1.0)).round() as u8,
    ]
}

fn render_text(
    fonts: &mut Fonts,
    doc: &Document,
    slide: &Slide,
    el: &TextBox,
    frame: &mut SlideFrame,
) -> Result<(), RenderError> {
    let p = frame.placement;
    let r = el.frame;
    let (x0, y0) = p.map(r.x, r.y);
    let box_w = (r.width as f32 * p.scale).max(1.0);
    let box_h = r.height as f32 * p.scale;

    let fill = el.fill.map(|f| with_opacity(f, el.opacity));
    if fill.is_some() && el.corner_radius > 0.0 {
        frame.warnings.push(warning(
            el,
            "rounded corners on the element fill aren't drawn yet".into(),
        ));
    }
    if el.problem == Some(TextProblem::NoStyledText) {
        frame.warnings.push(warning(
            el,
            "element has no RTFData; its text isn't drawn".into(),
        ));
    }
    let ctx = Ctx {
        file: &doc.path,
        slide: &slide.uuid,
        element: &el.uuid,
    };
    let layout = text::layout(fonts, el, p.scale, false, &ctx)?;
    let text_h = layout.as_ref().map_or(0.0, |l| l.text_h);
    if let Some(l) = &layout {
        for n in &l.notes {
            frame.warnings.push(warning(el, n.clone()));
        }
        if l.text_h > box_h + 0.5 {
            frame.warnings.push(RenderWarning {
                overflow: true,
                ..warning(
                    el,
                    format!(
                        "text is taller than its box ({:.0} vs {:.0} canvas px)",
                        l.text_h / p.scale,
                        r.height
                    ),
                )
            });
        }
    }
    let dy = text::valign_offset(el.valign, box_h, text_h);

    // Effects, in output pixels.
    let outline = el.outline.filter(|_| layout.is_some()).map(|o| {
        (
            (o.width as f32 * p.scale).max(0.0),
            with_opacity(o.color, el.opacity),
        )
    });
    let shadow = el.shadow.filter(|_| layout.is_some()).map(|s| {
        (
            (s.blur as f32 * p.scale).max(0.0),
            s.offset.0 as f32 * p.scale,
            s.offset.1 as f32 * p.scale,
            with_opacity(s.color, el.opacity),
        )
    });

    // 360° (seen in real files) is no rotation. Degrees clockwise (assumed; unverified
    // against PP6), about the box centre.
    let rotation = el.rotation.rem_euclid(360.0);
    if rotation == 0.0 && outline.is_none() && shadow.is_none() {
        let c = p.canvas;
        let mut layer = Layer {
            width: frame.width as usize,
            clip: (
                c.x as i32,
                c.y as i32,
                (c.x + c.width) as i32,
                (c.y + c.height) as i32,
            ),
            buf: &mut frame.text,
        };
        if let Some(color) = fill {
            layer.fill(x0, y0, box_w, box_h, color);
        }
        if let Some(mut l) = layout {
            layer.draw(fonts, &mut l, x0.round() as i32, (y0 + dy).round() as i32);
        }
        return Ok(());
    }

    // Layered: draw the box upright into its own layer (fill, shadow, outline,
    // text), then put it on the frame (rotated if needed).
    let reach =
        outline.map_or(0.0, |o| o.0) + shadow.map_or(0.0, |s| s.0 * 1.5 + s.1.abs().max(s.2.abs()));
    let margin = (4.0 + reach).ceil() as i32;
    // Integer layer origin in frame pixels, so upright text lands on the same pixels
    // as the direct path.
    let (ox, oy) = (
        x0.round() as i32 - margin,
        y0.round() as i32 + (dy.min(0.0).floor() as i32) - margin,
    );
    let bottom = (y0 + box_h.max(dy + text_h)).ceil() as i32 + margin;
    let right = (x0 + box_w).ceil() as i32 + margin;
    let (tw, th) = ((right - ox).max(1) as usize, (bottom - oy).max(1) as usize);
    let (text_x, text_y) = (x0.round() as i32 - ox, (y0 + dy).round() as i32 - oy);

    let mut text_layer = vec![0u8; tw * th * 4];
    if let Some(mut l) = layout {
        Layer {
            width: tw,
            clip: (0, 0, tw as i32, th as i32),
            buf: &mut text_layer,
        }
        .draw(fonts, &mut l, text_x, text_y);
    }
    let mut boxed = vec![0u8; tw * th * 4];
    {
        let mut layer = Layer {
            width: tw,
            clip: (0, 0, tw as i32, th as i32),
            buf: &mut boxed,
        };
        if let Some(color) = fill {
            layer.fill(x0 - ox as f32, y0 - oy as f32, box_w, box_h, color);
        }
    }
    if outline.is_some() || shadow.is_some() {
        let alpha: Vec<u8> = text_layer.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        let covered = match outline {
            Some((radius, _)) => effects::dilate(&alpha, tw, th, radius),
            None => alpha,
        };
        if let Some((blur, sx, sy, color)) = shadow {
            let soft = effects::blur(&covered, tw, th, blur);
            paint_mask(
                &mut boxed,
                tw,
                th,
                &soft,
                color,
                (sx.round() as i32, sy.round() as i32),
            );
        }
        if let Some((_, color)) = outline {
            paint_mask(&mut boxed, tw, th, &covered, color, (0, 0));
        }
    }
    over(&mut boxed, &text_layer);

    if rotation == 0.0 {
        composite_offset(frame, &boxed, (tw, th), (ox, oy));
    } else {
        composite_rotated(
            frame,
            &boxed,
            (tw, th),
            (x0 + box_w / 2.0 - ox as f32, y0 + box_h / 2.0 - oy as f32),
            (x0 + box_w / 2.0, y0 + box_h / 2.0),
            rotation as f32,
        );
    }
    Ok(())
}

fn media_warning(uuid: &str, message: String, media: bool) -> RenderWarning {
    RenderWarning {
        element_uuid: uuid.to_owned(),
        message,
        overflow: false,
        media,
    }
}

/// An image element: resized to its box at output scale, adjusted, then placed
/// like a text box's layer (rotated about its centre if needed).
fn render_image(doc: &Document, el: &ImageBox, frame: &mut SlideFrame) {
    let p = frame.placement;
    let r = el.frame;
    let (x0, y0) = p.map(r.x, r.y);
    let (bw, bh) = (r.width as f32 * p.scale, r.height as f32 * p.scale);
    let (w, h) = (bw.round().max(1.0) as u32, bh.round().max(1.0) as u32);
    if w > 16384 || h > 16384 {
        frame.warnings.push(media_warning(
            &el.uuid,
            "image is too large to draw".into(),
            false,
        ));
        return;
    }
    let sized = match picture::sized(&doc.path, &el.media, w, h, el.flip_h, el.flip_v) {
        Ok(s) => s,
        Err(e) => {
            frame
                .warnings
                .push(media_warning(&el.uuid, e.message(&el.media), true));
            return;
        }
    };
    for n in sized.notes {
        frame.warnings.push(media_warning(&el.uuid, n, false));
    }
    let px = picture::adjusted_premultiplied(&sized.image, &el.adjust, el.opacity);
    let rotation = el.rotation.rem_euclid(360.0);
    if rotation == 0.0 {
        composite_offset(
            frame,
            &px,
            (w as usize, h as usize),
            (x0.round() as i32, y0.round() as i32),
        );
    } else {
        composite_rotated(
            frame,
            &px,
            (w as usize, h as usize),
            (w as f32 / 2.0, h as f32 / 2.0),
            (x0 + bw / 2.0, y0 + bh / 2.0),
            rotation as f32,
        );
    }
}

/// The slide's background image over the whole canvas (Fit / Fill / Stretch),
/// adjusted over the canvas area (so a vignette frames the slide).
fn render_background_image(doc: &Document, bg: &BackgroundImage, frame: &mut SlideFrame) {
    let c = frame.placement.canvas;
    if c.width == 0 || c.height == 0 {
        return;
    }
    let path = bg.media.resolve(&doc.path);
    let (iw, ih) = match picture::image_size(&path) {
        Ok(s) => s,
        Err(_) if !path.exists() => {
            frame.warnings.push(media_warning(
                "",
                picture::ImageProblem::Missing(path).message(&bg.media),
                true,
            ));
            return;
        }
        Err(e) => {
            frame.warnings.push(media_warning(
                "",
                picture::ImageProblem::Unreadable(e).message(&bg.media),
                true,
            ));
            return;
        }
    };
    let (cw, ch) = (c.width as f32, c.height as f32);
    let (dw, dh) = match bg.fit {
        Fit::Stretch => (cw, ch),
        Fit::Fit | Fit::Fill => {
            let sx = cw / iw.max(1) as f32;
            let sy = ch / ih.max(1) as f32;
            let s = if bg.fit == Fit::Fit {
                sx.min(sy)
            } else {
                sx.max(sy)
            };
            (iw as f32 * s, ih as f32 * s)
        }
    };
    let (w, h) = (dw.round().max(1.0) as u32, dh.round().max(1.0) as u32);
    let sized = match picture::sized(&doc.path, &bg.media, w, h, false, false) {
        Ok(s) => s,
        Err(e) => {
            frame
                .warnings
                .push(media_warning("", e.message(&bg.media), true));
            return;
        }
    };
    for n in sized.notes {
        frame.warnings.push(media_warning("", n, false));
    }
    // Crop / pad into a canvas-sized picture, then adjust over the canvas.
    let (ox, oy) = (
        ((cw - w as f32) / 2.0).round() as i64,
        ((ch - h as f32) / 2.0).round() as i64,
    );
    let mut canvas = image::RgbaImage::new(c.width, c.height);
    {
        // Columns of the canvas covered by the picture, and where they start in it.
        let x_a = ox.max(0) as usize;
        let x_b = (ox + i64::from(w)).min(i64::from(c.width)).max(0) as usize;
        let (src, cw4, sw4) = (sized.image.as_raw(), c.width as usize * 4, w as usize * 4);
        let dst: &mut [u8] = &mut canvas;
        for y in 0..c.height as usize {
            let sy = y as i64 - oy;
            if sy < 0 || sy >= i64::from(h) || x_b <= x_a {
                continue;
            }
            let sx = (x_a as i64 - ox) as usize;
            let s0 = sy as usize * sw4 + sx * 4;
            let n = (x_b - x_a) * 4;
            dst[y * cw4 + x_a * 4..y * cw4 + x_a * 4 + n].copy_from_slice(&src[s0..s0 + n]);
        }
    }
    let px = picture::adjusted_premultiplied(&canvas, &bg.adjust, 1.0);
    composite_offset(
        frame,
        &px,
        (c.width as usize, c.height as usize),
        (c.x as i32, c.y as i32),
    );
}

/// Paint `color` through `mask` (shifted by `offset`) over a premultiplied layer.
fn paint_mask(dst: &mut [u8], w: usize, h: usize, mask: &[u8], color: [u8; 4], offset: (i32, i32)) {
    for y in 0..h as i32 {
        let sy = y - offset.1;
        if sy < 0 || sy >= h as i32 {
            continue;
        }
        for x in 0..w as i32 {
            let sx = x - offset.0;
            if sx < 0 || sx >= w as i32 {
                continue;
            }
            let m = mask[sy as usize * w + sx as usize];
            if m == 0 {
                continue;
            }
            let i = (y as usize * w + x as usize) * 4;
            blend(
                &mut dst[i..i + 4],
                [color[0], color[1], color[2], mul(color[3], m)],
            );
        }
    }
}

/// Premultiplied `src` over `dst` (same size).
fn over(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src.as_chunks::<4>().0)
    {
        if s[3] == 0 {
            continue;
        }
        let inv = 255 - s[3];
        for k in 0..4 {
            d[k] = s[k].saturating_add(mul(d[k], inv));
        }
    }
}

/// Premultiplied layer onto the frame at an integer offset, clipped to the canvas.
fn composite_offset(
    frame: &mut SlideFrame,
    src: &[u8],
    (sw, sh): (usize, usize),
    (ox, oy): (i32, i32),
) {
    let c = frame.placement.canvas;
    let width = frame.width as usize;
    for y in 0..sh as i32 {
        let fy = oy + y;
        if fy < c.y as i32 || fy >= (c.y + c.height) as i32 {
            continue;
        }
        for x in 0..sw as i32 {
            let fx = ox + x;
            if fx < c.x as i32 || fx >= (c.x + c.width) as i32 {
                continue;
            }
            let si = (y as usize * sw + x as usize) * 4;
            let a = src[si + 3];
            if a == 0 {
                continue;
            }
            let di = (fy as usize * width + fx as usize) * 4;
            let inv = 255 - a;
            for k in 0..4 {
                frame.text[di + k] = src[si + k].saturating_add(mul(frame.text[di + k], inv));
            }
        }
    }
}

/// A premultiplied RGBA pixel buffer with a clip rectangle `(x0, y0, x1, y1)`.
struct Layer<'a> {
    buf: &'a mut [u8],
    width: usize,
    clip: (i32, i32, i32, i32),
}

impl Layer<'_> {
    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        let xa = (x.round() as i32).max(cx0);
        let ya = (y.round() as i32).max(cy0);
        let xb = ((x + w).round() as i32).min(cx1);
        let yb = ((y + h).round() as i32).min(cy1);
        for yy in ya..yb {
            for xx in xa..xb {
                let i = (yy as usize * self.width + xx as usize) * 4;
                blend(&mut self.buf[i..i + 4], color);
            }
        }
    }

    /// Draw laid-out text (glyphs and underlines) with its top-left at `(ox, oy)`.
    fn draw(&mut self, fonts: &mut Fonts, l: &mut TextLayout, ox: i32, oy: i32) {
        let (cx0, cy0, cx1, cy1) = self.clip;
        let width = self.width;
        {
            let buf = &mut *self.buf;
            l.buffer.draw(
                &mut fonts.system,
                &mut fonts.swash,
                Color::rgba(0, 0, 0, 255),
                |x, y, w, h, color| {
                    let a = color.a();
                    if a == 0 {
                        return;
                    }
                    for yy in (oy + y).max(cy0)..(oy + y + h as i32).min(cy1) {
                        for xx in (ox + x).max(cx0)..(ox + x + w as i32).min(cx1) {
                            let i = (yy as usize * width + xx as usize) * 4;
                            blend(&mut buf[i..i + 4], [color.r(), color.g(), color.b(), a]);
                        }
                    }
                },
            );
        }
        // Underlines: a bar under each run of underlined glyphs, in the text colour.
        let mut bars: Vec<(f32, f32, f32, f32, Color)> = Vec::new();
        for run in l.buffer.layout_runs() {
            let mut cur: Option<(f32, f32, f32, f32, Color, usize)> = None;
            for g in run.glyphs {
                if !l.underline.get(g.metadata).copied().unwrap_or(false) {
                    if let Some(b) = cur.take() {
                        bars.push((b.0, b.1, b.2, b.3, b.4));
                    }
                    continue;
                }
                let y = run.line_y + g.font_size * 0.12;
                let t = (g.font_size * 0.06).max(1.0);
                let color = l.colors[g.metadata];
                match &mut cur {
                    Some(b) if b.5 == g.metadata && (b.1 - g.x).abs() < 1.0 => b.1 = g.x + g.w,
                    _ => {
                        if let Some(b) = cur.take() {
                            bars.push((b.0, b.1, b.2, b.3, b.4));
                        }
                        cur = Some((g.x, g.x + g.w, y, t, color, g.metadata));
                    }
                }
            }
            if let Some(b) = cur {
                bars.push((b.0, b.1, b.2, b.3, b.4));
            }
        }
        for (xa, xb, y, t, c) in bars {
            self.fill(
                ox as f32 + xa,
                oy as f32 + y,
                xb - xa,
                t,
                [c.r(), c.g(), c.b(), c.a()],
            );
        }
    }
}

/// Composite a premultiplied layer onto the frame, rotated `degrees` clockwise so
/// that the layer point `pivot_src` lands on frame point `pivot_dst`. Bilinear
/// sampling, clipped to the canvas.
fn composite_rotated(
    frame: &mut SlideFrame,
    src: &[u8],
    (sw, sh): (usize, usize),
    pivot_src: (f32, f32),
    pivot_dst: (f32, f32),
    degrees: f32,
) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    // Frame-space bounds of the rotated layer.
    let corners = [
        (0.0, 0.0),
        (sw as f32, 0.0),
        (0.0, sh as f32),
        (sw as f32, sh as f32),
    ];
    let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in corners {
        let (dx, dy) = (x - pivot_src.0, y - pivot_src.1);
        let fx = pivot_dst.0 + dx * cos - dy * sin;
        let fy = pivot_dst.1 + dx * sin + dy * cos;
        bx0 = bx0.min(fx);
        by0 = by0.min(fy);
        bx1 = bx1.max(fx);
        by1 = by1.max(fy);
    }
    let c = frame.placement.canvas;
    let xa = (bx0.floor() as i64).max(i64::from(c.x));
    let ya = (by0.floor() as i64).max(i64::from(c.y));
    let xb = (bx1.ceil() as i64).min(i64::from(c.x + c.width));
    let yb = (by1.ceil() as i64).min(i64::from(c.y + c.height));
    let at = |x: i64, y: i64| -> [f32; 4] {
        if x < 0 || y < 0 || x >= sw as i64 || y >= sh as i64 {
            return [0.0; 4];
        }
        let i = (y as usize * sw + x as usize) * 4;
        [
            f32::from(src[i]),
            f32::from(src[i + 1]),
            f32::from(src[i + 2]),
            f32::from(src[i + 3]),
        ]
    };
    let width = frame.width as usize;
    for yy in ya..yb {
        for xx in xa..xb {
            // Inverse-rotate the pixel centre into layer space.
            let (dx, dy) = (xx as f32 + 0.5 - pivot_dst.0, yy as f32 + 0.5 - pivot_dst.1);
            let lx = pivot_src.0 + dx * cos + dy * sin - 0.5;
            let ly = pivot_src.1 - dx * sin + dy * cos - 0.5;
            let (fx, fy) = (lx.floor(), ly.floor());
            let (tx, ty) = (lx - fx, ly - fy);
            let (ix, iy) = (fx as i64, fy as i64);
            let (p00, p10, p01, p11) = (
                at(ix, iy),
                at(ix + 1, iy),
                at(ix, iy + 1),
                at(ix + 1, iy + 1),
            );
            let mut px = [0u8; 4];
            for k in 0..4 {
                let top = p00[k] + (p10[k] - p00[k]) * tx;
                let bot = p01[k] + (p11[k] - p01[k]) * tx;
                px[k] = (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8;
            }
            if px[3] == 0 {
                continue;
            }
            let i = (yy as usize * width + xx as usize) * 4;
            let inv = 255 - px[3];
            let d = &mut frame.text[i..i + 4];
            for k in 0..4 {
                d[k] = px[k].saturating_add(mul(d[k], inv));
            }
        }
    }
}

/// Source-over blend of a straight-alpha colour onto a premultiplied pixel.
fn blend(dst: &mut [u8], [r, g, b, a]: [u8; 4]) {
    let inv = 255 - a;
    dst[0] = mul(r, a).saturating_add(mul(dst[0], inv));
    dst[1] = mul(g, a).saturating_add(mul(dst[1], inv));
    dst[2] = mul(b, a).saturating_add(mul(dst[2], inv));
    dst[3] = a.saturating_add(mul(dst[3], inv));
}
