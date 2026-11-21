//! Text effects on alpha masks: outline (round dilation via a Euclidean distance
//! transform, linear in the number of pixels) and blur for drop shadows (three box
//! blur passes, close to a Gaussian).

const INF: f64 = 1e20;

/// One-dimensional squared distance transform (Felzenszwalb & Huttenlocher).
fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let parab = |q: usize, p: usize| {
        ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64)
    };
    let mut k = 0usize;
    v[0] = 0;
    z[0] = -INF;
    z[1] = INF;
    for q in 1..n {
        let mut s = parab(q, v[k]);
        while k > 0 && s <= z[k] {
            k -= 1;
            s = parab(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = INF;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let dq = q as f64 - v[k] as f64;
        *out = dq * dq + f[v[k]];
    }
}

/// Squared distance from every pixel to the nearest pixel with alpha ≥ 128.
fn distance_sq(alpha: &[u8], w: usize, h: usize) -> Vec<f64> {
    let mut grid: Vec<f64> = alpha
        .iter()
        .map(|&a| if a >= 128 { 0.0 } else { INF })
        .collect();
    let n = w.max(h);
    let (mut f, mut d) = (vec![0f64; n], vec![0f64; n]);
    let (mut v, mut z) = (vec![0usize; n], vec![0f64; n + 1]);
    for x in 0..w {
        for y in 0..h {
            f[y] = grid[y * w + x];
        }
        edt_1d(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            grid[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&grid[y * w..(y + 1) * w]);
        edt_1d(&f[..w], &mut d[..w], &mut v, &mut z);
        grid[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    grid
}

/// The mask grown by `radius` pixels (round corners), antialiased at the new edge.
pub(crate) fn dilate(alpha: &[u8], w: usize, h: usize, radius: f32) -> Vec<u8> {
    if radius <= 0.0 || w == 0 || h == 0 {
        return alpha.to_vec();
    }
    let d2 = distance_sq(alpha, w, h);
    alpha
        .iter()
        .zip(&d2)
        .map(|(&a, &d)| {
            let grown =
                ((f64::from(radius) + 0.5 - d.sqrt()).clamp(0.0, 1.0) * 255.0).round() as u8;
            a.max(grown)
        })
        .collect()
}

fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, count) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| {
        if horizontal {
            line * w + i
        } else {
            i * w + line
        }
    };
    let div = (2 * r + 1) as u32;
    for line in 0..count {
        let mut sum: u32 = 0;
        // Window [i - r, i + r], edges count as 0.
        for i in 0..=r.min(len.saturating_sub(1)) {
            sum += u32::from(src[idx(line, i)]);
        }
        for i in 0..len {
            dst[idx(line, i)] = (sum / div) as u8;
            let add = i + r + 1;
            if add < len {
                sum += u32::from(src[idx(line, add)]);
            }
            if i >= r {
                sum -= u32::from(src[idx(line, i - r)]);
            }
        }
    }
}

/// Blur the mask by about `radius` pixels.
pub(crate) fn blur(alpha: &[u8], w: usize, h: usize, radius: f32) -> Vec<u8> {
    let r = (radius * 0.6).round() as usize;
    if r == 0 || w == 0 || h == 0 {
        return alpha.to_vec();
    }
    let mut a = alpha.to_vec();
    let mut b = vec![0u8; a.len()];
    for _ in 0..3 {
        box_pass(&a, &mut b, w, h, r, true);
        box_pass(&b, &mut a, w, h, r, false);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dilation_grows_a_dot_into_a_round_disc() {
        let (w, h) = (21, 21);
        let mut a = vec![0u8; w * h];
        a[10 * w + 10] = 255;
        let d = dilate(&a, w, h, 5.0);
        assert_eq!(d[10 * w + 14], 255, "4 px right: inside");
        assert_eq!(d[10 * w + 15], 128, "5 px right: the antialiased edge");
        assert_eq!(d[10 * w + 16], 0, "6 px right: outside");
        // Corner at distance 5·√2 ≈ 7.07: outside a round disc of radius 5.
        assert_eq!(d[15 * w + 15], 0);
        assert_eq!(d[13 * w + 13], 255, "distance 4.2");
    }

    #[test]
    fn blur_spreads_and_keeps_the_total_roughly() {
        let (w, h) = (40, 40);
        let mut a = vec![0u8; w * h];
        for y in 15..25 {
            for x in 15..25 {
                a[y * w + x] = 255;
            }
        }
        let b = blur(&a, w, h, 6.0);
        assert!(b[20 * w + 20] > 100, "centre stays strong");
        assert!(b[20 * w + 12] > 0, "spreads outwards");
        let total = |v: &[u8]| v.iter().map(|&x| u64::from(x)).sum::<u64>();
        let (t0, t1) = (total(&a), total(&b));
        assert!(t1 * 100 >= t0 * 85 && t1 * 100 <= t0 * 105, "{t0} vs {t1}");
    }
}
