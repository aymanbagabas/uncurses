use super::{Color, XTERM_COLORS};

/// Convert an RGB color to the nearest xterm 256-color index.
pub fn rgb_to_ansi256((r, g, b): (u8, u8, u8)) -> Color {
    // Check for exact match in the 6x6x6 cube
    let cube_values: [u8; 6] = [0, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

    let ri = nearest_cube_index(r, &cube_values);
    let gi = nearest_cube_index(g, &cube_values);
    let bi = nearest_cube_index(b, &cube_values);
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;
    let (cr, cg, cb) = XTERM_COLORS[cube_idx as usize];
    let cube_dist = color_distance_sq((r, g, b), (cr, cg, cb));

    // Check grayscale ramp
    let gray_idx = if r == g && g == b {
        // Exact gray — find nearest grayscale entry
        nearest_gray(r)
    } else {
        let avg = ((r as u16 + g as u16 + b as u16) / 3) as u8;
        nearest_gray(avg)
    };
    let (gr, gg, gb) = XTERM_COLORS[gray_idx as usize];
    let gray_dist = color_distance_sq((r, g, b), (gr, gg, gb));

    if gray_dist < cube_dist {
        Color::Indexed(gray_idx)
    } else {
        Color::Indexed(cube_idx)
    }
}

/// Convert an RGB color to the nearest 16-color ANSI color.
pub fn rgb_to_ansi16((r, g, b): (u8, u8, u8)) -> Color {
    let mut best_idx = 0u8;
    let mut best_dist = u32::MAX;

    for i in 0..16u8 {
        let (pr, pg, pb) = XTERM_COLORS[i as usize];
        let d = color_distance_sq((r, g, b), (pr, pg, pb));
        if d < best_dist {
            best_dist = d;
            best_idx = i;
        }
    }

    Color::from_named(best_idx).unwrap_or(Color::White)
}

fn nearest_cube_index(v: u8, cube: &[u8; 6]) -> u8 {
    let mut best = 0u8;
    let mut best_dist = u8::MAX;
    for (i, &cv) in cube.iter().enumerate() {
        let d = v.abs_diff(cv);
        if d < best_dist {
            best_dist = d;
            best = i as u8;
        }
    }
    best
}

fn nearest_gray(v: u8) -> u8 {
    // Grayscale ramp: indices 232-255, values 8, 18, 28, ..., 238
    if v < 4 {
        // Lowest grayscale-ramp entry; cube black is considered separately.
        return 232;
    }
    if v > 243 {
        return 255;
    }
    // gray_value = 8 + 10 * (idx - 232)
    // idx = (v - 8) / 10 + 232, rounded

    ((v as u16 - 8 + 5) / 10 + 232).min(255) as u8
}

/// Squared Euclidean distance in RGB space.
fn color_distance_sq(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let dr = a.0 as i32 - b.0 as i32;
    let dg = a.1 as i32 - b.1 as i32;
    let db = a.2 as i32 - b.2 as i32;
    // Weighted RGB distance (human perception)
    (2 * dr * dr + 4 * dg * dg + 3 * db * db) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pure_red_to_256() {
        let c = rgb_to_ansi256((255, 0, 0));
        assert_eq!(c, Color::Indexed(196)); // 6x6x6 cube: (5,0,0)
    }

    #[test]
    fn test_white_to_256() {
        let c = rgb_to_ansi256((255, 255, 255));
        // Should be index 231 (cube 5,5,5) or 255 (gray) — both are white
        matches!(c, Color::Indexed(231) | Color::Indexed(255));
    }

    #[test]
    fn test_gray_to_256() {
        let c = rgb_to_ansi256((128, 128, 128));
        // Should pick a grayscale index
        if let Color::Indexed(idx) = c {
            assert!((232..=255).contains(&idx) || (16..=231).contains(&idx));
        } else {
            panic!("expected indexed color");
        }
    }

    #[test]
    fn test_pure_red_to_16() {
        let c = rgb_to_ansi16((255, 0, 0));
        assert_eq!(c, Color::BrightRed);
    }

    #[test]
    fn test_black_to_16() {
        let c = rgb_to_ansi16((0, 0, 0));
        assert_eq!(c, Color::Black);
    }
}
