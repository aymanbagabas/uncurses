use std::collections::BTreeMap;
use std::sync::LazyLock;

use font8x8::UnicodeFonts;
use uncurses::text::WidthMode;

use super::Tags;

pub(super) struct Glyph {
    pub character: char,
    pub width: u8,
    pub mask: [u64; 2],
    pub tags: Tags,
}

pub(super) const POLICIES: [(WidthMode, bool); 4] = [
    (WidthMode::Wc, false),
    (WidthMode::Wc, true),
    (WidthMode::Grapheme, false),
    (WidthMode::Grapheme, true),
];

pub(super) static GLYPHS: LazyLock<Vec<Glyph>> = LazyLock::new(|| build(WidthMode::Wc, false));

pub(super) fn glyphs(mode: WidthMode, eaw_wide: bool) -> &'static [Glyph] {
    static WC_WIDE: LazyLock<Vec<Glyph>> = LazyLock::new(|| build(WidthMode::Wc, true));
    static GRAPHEME: LazyLock<Vec<Glyph>> = LazyLock::new(|| build(WidthMode::Grapheme, false));
    static GRAPHEME_WIDE: LazyLock<Vec<Glyph>> = LazyLock::new(|| build(WidthMode::Grapheme, true));
    match (mode, eaw_wide) {
        (WidthMode::Wc, false) => &GLYPHS,
        (WidthMode::Wc, true) => &WC_WIDE,
        (WidthMode::Grapheme, false) => &GRAPHEME,
        (WidthMode::Grapheme, true) => &GRAPHEME_WIDE,
    }
}

fn mask(mut covered: impl FnMut(i32, i32) -> bool) -> u64 {
    let mut bits = 0;
    for y in 0..8 {
        for x in 0..8 {
            if covered(x, y) {
                bits |= 1 << (y * 8 + x);
            }
        }
    }
    bits
}

fn mosaic(bits: u8, rows: i32) -> u64 {
    mask(|x, y| bits & (1 << (((2 * y + 1) * rows / 16) * 2 + x / 4)) != 0)
}

struct Catalog {
    glyphs: BTreeMap<char, Glyph>,
    mode: WidthMode,
    eaw_wide: bool,
}

impl Catalog {
    fn add(&mut self, character: char, bits: u64, tags: Tags) {
        let width = self
            .mode
            .grapheme_width(character.encode_utf8(&mut [0; 4]), self.eaw_wide);
        let width_tag = match width {
            1 => Tags::NARROW,
            2 => Tags::WIDE,
            _ => return,
        };
        let masks = if width == 2 {
            std::array::from_fn(|j| {
                mask(|x, y| bits & (1 << (y * 8 + (x + j as i32 * 8) / 2)) != 0)
            })
        } else {
            [bits, 0]
        };
        self.glyphs
            .entry(character)
            .and_modify(|g| g.tags |= tags)
            .or_insert(Glyph {
                character,
                width,
                mask: masks,
                tags: tags | width_tag,
            });
    }

    fn font(&mut self, font: &impl UnicodeFonts, tags: Tags) {
        for glyph in font.iter() {
            let character = glyph.char();
            if character.is_control()
                || character == '\u{7f}'
                || character == '\u{03a2}'
                || (glyph.is_whitespace() && !character.is_whitespace())
            {
                continue;
            }
            let rows = glyph.byte_array();
            let bits = u64::from_le_bytes(rows);
            let mut tags = tags;
            if character.is_alphabetic() {
                tags |= Tags::ALPHA;
            }
            if character.is_ascii_digit() {
                tags |= Tags::DIGIT;
            }
            self.add(character, bits, tags);
        }
    }
}

fn build(mode: WidthMode, eaw_wide: bool) -> Vec<Glyph> {
    use Tags as S;
    let mut out = Catalog {
        glyphs: BTreeMap::new(),
        mode,
        eaw_wide,
    };
    let block = S::BLOCK;
    out.add(
        ' ',
        0,
        S::SPACE | block | S::HALF | S::QUADRANT | S::SEXTANT | S::OCTANT,
    );
    out.add(
        '\u{2588}',
        u64::MAX,
        S::SOLID | block | S::HALF | S::QUADRANT | S::SEXTANT | S::OCTANT,
    );
    out.add(
        '\u{2580}',
        mask(|_, y| y < 4),
        block | S::UPPER_LOWER | S::QUADRANT | S::OCTANT,
    );
    out.add(
        '\u{2584}',
        mask(|_, y| y >= 4),
        block | S::UPPER_LOWER | S::QUADRANT | S::OCTANT,
    );
    out.add(
        '\u{258c}',
        mask(|x, _| x < 4),
        block | S::LEFT_RIGHT | S::QUADRANT | S::SEXTANT | S::OCTANT,
    );
    out.add(
        '\u{2590}',
        mask(|x, _| x >= 4),
        block | S::LEFT_RIGHT | S::QUADRANT | S::SEXTANT | S::OCTANT,
    );
    for n in 1..=8 {
        out.add(
            char::from_u32(0x2580 + n).unwrap(),
            mask(|_, y| y >= 8 - n as i32),
            block,
        );
        out.add(
            char::from_u32(0x2590 - n).unwrap(),
            mask(|x, _| x < n as i32),
            block,
        );
    }
    out.add('\u{2594}', mask(|_, y| y == 0), block);
    out.add('\u{2595}', mask(|x, _| x == 7), block);
    for (ch, bits) in [
        ('\u{2596}', 4),
        ('\u{2597}', 8),
        ('\u{2598}', 1),
        ('\u{2599}', 13),
        ('\u{259a}', 9),
        ('\u{259b}', 7),
        ('\u{259c}', 11),
        ('\u{259d}', 2),
        ('\u{259e}', 6),
        ('\u{259f}', 14),
    ] {
        out.add(ch, mosaic(bits, 2), block | S::QUADRANT);
    }
    for n in 1..=3 {
        out.add(
            char::from_u32(0x2590 + n).unwrap(),
            mask(|x, y| (x + 2 * (y % 2)) % 4 < n as i32),
            block | S::STIPPLE,
        );
    }
    out.font(&font8x8::BASIC_FONTS, S::ASCII | S::LATIN);
    out.font(&font8x8::LATIN_FONTS, S::LATIN);
    out.font(&font8x8::GREEK_FONTS, S::TECHNICAL);
    out.font(&font8x8::HIRAGANA_FONTS, S::empty());
    out.font(&font8x8::BOX_FONTS, S::BORDER);
    for (ch, bits) in [
        ('\u{2571}', mask(|x, y| x + y == 7)),
        ('\u{2572}', mask(|x, y| x == y)),
        ('\u{2573}', mask(|x, y| x == y || x + y == 7)),
    ] {
        // Override the font's diagonal masks with geometric full-cell strokes.
        out.glyphs.remove(&ch);
        out.add(ch, bits, S::DIAGONAL | S::BORDER);
    }
    for (ch, bits) in [
        (
            '\u{00b7}',
            mask(|x, y| (3..5).contains(&x) && (3..5).contains(&y)),
        ),
        (
            '\u{2022}',
            mask(|x, y| (2..6).contains(&x) && (2..6).contains(&y)),
        ),
        ('\u{2219}', mask(|x, y| x == 3 && y == 3)),
    ] {
        out.add(ch, bits, S::DOT);
    }
    for (ch, bits) in [
        ('\u{2212}', mask(|x, y| (1..7).contains(&x) && y == 3)),
        (
            '\u{00d7}',
            mask(|x, y| (1..7).contains(&x) && (x == y || x + y == 7)),
        ),
        (
            '\u{00f7}',
            mask(|x, y| ((1..7).contains(&x) && y == 3) || (x == 3 && (y == 1 || y == 5))),
        ),
        (
            '\u{2261}',
            mask(|x, y| (1..7).contains(&x) && matches!(y, 1 | 3 | 5)),
        ),
        (
            '\u{2260}',
            mask(|x, y| ((1..7).contains(&x) && matches!(y, 2 | 5)) || x + y == 7),
        ),
        (
            '\u{22a5}',
            mask(|x, y| (y == 6 && (1..7).contains(&x)) || (x == 3 && (1..7).contains(&y))),
        ),
        (
            '\u{2310}',
            mask(|x, y| (x >= 2 && y == 2) || (x == 2 && (2..6).contains(&y))),
        ),
    ] {
        out.add(ch, bits, S::TECHNICAL);
    }
    for (ch, bits) in [
        (
            '\u{25a0}',
            mask(|x, y| (1..7).contains(&x) && (1..7).contains(&y)),
        ),
        (
            '\u{25a1}',
            mask(|x, y| {
                (1..7).contains(&x) && (1..7).contains(&y) && (x == 1 || x == 6 || y == 1 || y == 6)
            }),
        ),
        (
            '\u{25cf}',
            mask(|x, y| (2 * x - 7).pow(2) + (2 * y - 7).pow(2) <= 40),
        ),
        (
            '\u{25cb}',
            mask(|x, y| (24..=48).contains(&((2 * x - 7).pow(2) + (2 * y - 7).pow(2)))),
        ),
        (
            '\u{25c6}',
            mask(|x, y| (2 * x - 7).abs() + (2 * y - 7).abs() <= 7),
        ),
        (
            '\u{25c7}',
            mask(|x, y| (2 * x - 7).abs() + (2 * y - 7).abs() == 6),
        ),
        ('\u{25b2}', mask(|x, y| (2 * x - 7).abs() <= y && y < 7)),
        ('\u{25bc}', mask(|x, y| (2 * x - 7).abs() <= 7 - y && y > 0)),
        ('\u{25c0}', mask(|x, y| (2 * y - 7).abs() <= x && x < 7)),
        ('\u{25b6}', mask(|x, y| (2 * y - 7).abs() <= 7 - x && x > 0)),
    ] {
        out.add(ch, bits, S::GEOMETRIC);
    }
    for pattern in 0..=255u32 {
        let dots = [
            (1, 0),
            (1, 2),
            (1, 4),
            (5, 0),
            (5, 2),
            (5, 4),
            (1, 6),
            (5, 6),
        ];
        let bits = mask(|x, y| {
            dots.iter().enumerate().any(|(bit, &(dx, dy))| {
                pattern & (1 << bit) != 0 && (dx..dx + 2).contains(&x) && (dy..dy + 2).contains(&y)
            })
        });
        out.add(char::from_u32(0x2800 + pattern).unwrap(), bits, S::BRAILLE);
    }
    for pattern in 1..63u8 {
        if pattern == 21 || pattern == 42 {
            continue;
        }
        let code =
            0x1fb00 + u32::from(pattern - 1 - u8::from(pattern > 21) - u8::from(pattern > 42));
        out.add(
            char::from_u32(code).unwrap(),
            mosaic(pattern, 3),
            S::SEXTANT | S::LEGACY,
        );
    }
    // Octant code points omit shapes already encoded elsewhere in Unicode.
    let reused = [
        (0, ' '),
        (1, '\u{1cea8}'),
        (2, '\u{1ceab}'),
        (3, '\u{1fb82}'),
        (5, '\u{2598}'),
        (10, '\u{259d}'),
        (15, '\u{2580}'),
        (20, '\u{1fbe6}'),
        (40, '\u{1fbe7}'),
        (63, '\u{1fb85}'),
        (64, '\u{1cea3}'),
        (80, '\u{2596}'),
        (85, '\u{258c}'),
        (90, '\u{259e}'),
        (95, '\u{259b}'),
        (128, '\u{1cea0}'),
        (160, '\u{2597}'),
        (165, '\u{259a}'),
        (170, '\u{2590}'),
        (175, '\u{259c}'),
        (192, '\u{2582}'),
        (240, '\u{2584}'),
        (245, '\u{2599}'),
        (250, '\u{259f}'),
        (252, '\u{2586}'),
        (255, '\u{2588}'),
    ];
    let mut next = 0x1cd00;
    for pattern in 0..=255u8 {
        let character = if let Some(&(_, ch)) = reused.iter().find(|&&(bits, _)| bits == pattern) {
            ch
        } else {
            let ch = char::from_u32(next).unwrap();
            next += 1;
            ch
        };
        out.add(character, mosaic(pattern, 4), S::OCTANT | S::LEGACY);
    }
    wedges(&mut out);
    legacy(&mut out);
    let mut glyphs: Vec<_> = out.glyphs.into_values().collect();
    for i in 0..glyphs.len() {
        if glyphs[..i].iter().any(|g| {
            g.width == glyphs[i].width
                && (0..usize::from(g.width)).all(|j| g.mask[j] == !glyphs[i].mask[j])
        }) {
            glyphs[i].tags |= S::INVERTED;
        }
    }
    glyphs
}

fn wedges(out: &mut Catalog) {
    let tags = Tags::WEDGE | Tags::LEGACY;
    // Endpoints use sixths of a cell, so third-height edges remain exact.
    let edges = [
        ((0, 4), (3, 6), (0, 6)),
        ((0, 4), (6, 6), (0, 6)),
        ((0, 2), (3, 6), (0, 6)),
        ((0, 2), (6, 6), (0, 6)),
        ((0, 0), (3, 6), (0, 6)),
        ((0, 2), (3, 0), (6, 6)),
        ((0, 2), (6, 0), (6, 6)),
        ((0, 4), (3, 0), (6, 6)),
        ((0, 4), (6, 0), (6, 6)),
        ((0, 6), (3, 0), (6, 6)),
        ((0, 4), (6, 2), (6, 6)),
        ((3, 6), (6, 4), (6, 6)),
        ((0, 6), (6, 4), (6, 6)),
        ((3, 6), (6, 2), (6, 6)),
        ((0, 6), (6, 2), (6, 6)),
        ((3, 6), (6, 0), (6, 6)),
        ((3, 0), (6, 2), (0, 6)),
        ((0, 0), (6, 2), (0, 6)),
        ((3, 0), (6, 4), (0, 6)),
        ((0, 0), (6, 4), (0, 6)),
        ((3, 0), (6, 6), (0, 6)),
        ((0, 2), (6, 4), (0, 6)),
    ];
    for (i, (a, b, inside)) in edges.into_iter().enumerate() {
        let cross = |x: i32, y: i32| (b.0 - a.0) * (y - a.1 * 16) - (b.1 - a.1) * (x - a.0 * 16);
        let sign = cross(inside.0 * 16, inside.1 * 16).signum();
        let bits = mask(|x, y| cross((2 * x + 1) * 6, (2 * y + 1) * 6) * sign >= 0);
        out.add(char::from_u32(0x1fb3c + i as u32).unwrap(), bits, tags);
        out.add(char::from_u32(0x1fb52 + i as u32).unwrap(), !bits, tags);
    }
    for (i, bits) in [
        mask(|x, y| x < y && x + y < 7),
        mask(|x, y| y <= x && x + y < 7),
        mask(|x, y| x > y && x + y >= 7),
        mask(|x, y| y >= x && x + y >= 7),
    ]
    .into_iter()
    .enumerate()
    {
        out.add(char::from_u32(0x1fb6c + i as u32).unwrap(), bits, tags);
        out.add(char::from_u32(0x1fb68 + i as u32).unwrap(), !bits, tags);
    }
}

fn legacy(out: &mut Catalog) {
    let tags = Tags::LEGACY;
    for i in 1..7 {
        out.add(
            char::from_u32(0x1fb6f + i).unwrap(),
            mask(|x, _| x == i as i32),
            tags,
        );
        out.add(
            char::from_u32(0x1fb75 + i).unwrap(),
            mask(|_, y| y == i as i32),
            tags,
        );
    }
    for (i, n) in [2, 3, 5, 6, 7].into_iter().enumerate() {
        out.add(
            char::from_u32(0x1fb82 + i as u32).unwrap(),
            mask(|_, y| y < n),
            tags,
        );
        out.add(
            char::from_u32(0x1fb87 + i as u32).unwrap(),
            mask(|x, _| x >= 8 - n),
            tags,
        );
    }
    for (code, bits) in [
        (0x1fb7c, mask(|x, y| x == 0 || y == 7)),
        (0x1fb7d, mask(|x, y| x == 0 || y == 0)),
        (0x1fb7e, mask(|x, y| x == 7 || y == 0)),
        (0x1fb7f, mask(|x, y| x == 7 || y == 7)),
        (0x1fb80, mask(|_, y| y == 0 || y == 7)),
        (0x1fb81, mask(|_, y| matches!(y, 0 | 2 | 4 | 7))),
        (0x1fb8c, mask(|x, y| x < 4 && (x + y) % 2 == 0)),
        (0x1fb8d, mask(|x, y| x >= 4 && (x + y) % 2 == 0)),
        (0x1fb8e, mask(|x, y| y < 4 && (x + y) % 2 == 0)),
        (0x1fb8f, mask(|x, y| y >= 4 && (x + y) % 2 == 0)),
        (0x1fb90, mask(|x, y| (x + y) % 2 == 1)),
        (0x1fb91, mask(|x, y| y < 4 || (x + y) % 2 == 1)),
        (0x1fb92, mask(|x, y| y >= 4 || (x + y) % 2 == 1)),
        (0x1fb94, mask(|x, y| x >= 4 || (x + y) % 2 == 1)),
        (0x1fb95, mask(|x, y| (x / 2 + y / 2) % 2 == 0)),
        (0x1fb96, mask(|x, y| (x / 2 + y / 2) % 2 == 1)),
        (0x1fb97, mask(|_, y| y % 4 < 2)),
        (0x1fb98, mask(|x, y| (x - y).rem_euclid(4) < 2)),
        (0x1fb99, mask(|x, y| (x + y) % 4 < 2)),
        (
            0x1fb9a,
            mask(|x, y| y < x && x + y < 7 || y >= x && x + y >= 7),
        ),
        (
            0x1fb9b,
            mask(|x, y| x <= y && x + y < 7 || x > y && x + y >= 7),
        ),
    ] {
        out.add(char::from_u32(code).unwrap(), bits, tags);
    }
}
