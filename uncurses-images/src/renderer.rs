//! Automatic protocol selection and screen-scoped image resources.

use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::Arc,
};

use uncurses::{
    color::Profile,
    event::Input,
    layout::{Rect, Size},
    program::Program,
    screen::{PlacementId, PlacementOptions, Screen},
    text::{TextSurface, WidthMode},
};

use crate::{Backend, Image, ImageOptions, SymbolSet, invalid};

/// Image protocols and physical geometry available to one terminal.
///
/// Set flags only for protocols the terminal supports. Placeholder support
/// includes underline colors. Sixel support includes cursor-relative output
/// and graphics erasure through character operations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImageCapabilities {
    /// Kitty Unicode placeholders with underline colors.
    pub kitty_placeholders: bool,
    /// Native Kitty placements.
    pub kitty: bool,
    /// iTerm2 inline images.
    pub iterm2: bool,
    /// Sixel with cursor-relative output and cell erasure.
    pub sixel: bool,
    /// Actual cell dimensions in pixels. Missing or zero dimensions select symbols.
    pub cell_pixels: Option<Size>,
}

impl ImageCapabilities {
    /// Read the program's current reports and terminal environment.
    ///
    /// Process capability replies when available. Kitty graphics uses a query
    /// reply or a Kitty, Rio, or Ghostty identity. Placeholders require Rio,
    /// Ghostty, or a reported Kitty version of at least 0.28.
    /// Sixel skips the first primary DA parameter and looks for capability 4
    /// among the remaining parameters.
    /// iTerm2 images use iTerm2, Rio, WezTerm, mintty, or Konsole identities.
    /// Identity precedence is the reported name, nonempty `TERM_PROGRAM`, then
    /// an exact `TERM` match: `xterm-kitty`, `xterm-ghostty`, `xterm-rio`, `rio`,
    /// or `wezterm`. Generic values such as `xterm-256color` are inconclusive.
    /// Optional image support, such as VS Code's, requires explicit flags.
    /// In tmux and GNU Screen, automatic selection permits placeholders and symbols.
    /// [`ImageRenderer::from_program`] wraps placeholder resources for tmux.
    /// GNU Screen (`STY`) and other transports require explicit resource
    /// forwarding. This method sends no queries.
    pub fn from_program<I: Input, W: Write>(program: &Program<I, W>) -> Self {
        let caps = program.capabilities();
        let multiplexed = ["TMUX", "STY"].iter().any(|key| {
            program
                .env()
                .get(key)
                .is_some_and(|value| !value.is_empty())
        });
        let name = program.terminal_name().unwrap_or("");
        let term_program = program.env().get("TERM_PROGRAM");
        let term = program.env().get("TERM");
        let identity = terminal_identity(name, term_program.as_deref(), term.as_deref());
        let kitty = caps.kitty_graphics() || kitty(identity);
        Self {
            kitty_placeholders: kitty
                && kitty_placeholders(if name.is_empty() { identity } else { name }),
            kitty: !multiplexed && kitty,
            iterm2: !multiplexed && iterm2(identity),
            sixel: !multiplexed && sixel(caps.primary_device_attributes()),
            cell_pixels: program.cell_pixels(),
        }
    }

    fn pixels(self) -> Option<Size> {
        self.cell_pixels
            .filter(|size| size.width > 0 && size.height > 0)
    }
}

fn terminal_identity<'a>(
    name: &'a str,
    term_program: Option<&'a str>,
    term: Option<&str>,
) -> &'a str {
    if !name.is_empty() {
        return name.split([' ', '(']).next().unwrap_or("");
    }
    if let Some(program) = term_program.filter(|program| !program.is_empty()) {
        return program;
    }
    match term {
        Some("xterm-kitty") => "kitty",
        Some("xterm-ghostty") => "ghostty",
        Some("xterm-rio" | "rio") => "rio",
        Some("wezterm") => "wezterm",
        _ => "",
    }
}

fn kitty(identity: &str) -> bool {
    ["kitty", "Rio", "Ghostty"]
        .iter()
        .any(|supported| identity.eq_ignore_ascii_case(supported))
}

fn iterm2(identity: &str) -> bool {
    ["iTerm2", "iTerm.app", "Rio", "WezTerm", "mintty", "Konsole"]
        .iter()
        .any(|supported| identity.eq_ignore_ascii_case(supported))
}

fn kitty_placeholders(name: &str) -> bool {
    let identity = name.split([' ', '(']).next().unwrap_or("");
    if ["Rio", "Ghostty"]
        .iter()
        .any(|supported| identity.eq_ignore_ascii_case(supported))
    {
        return true;
    }
    let Some(version) = name
        .strip_prefix("kitty(")
        .and_then(|s| s.strip_suffix(')'))
    else {
        return false;
    };
    let mut parts = version.split('.');
    let major = parts.next().and_then(|s| s.parse::<u32>().ok());
    let minor = parts.next().and_then(|s| s.parse::<u32>().ok());
    matches!((major, minor), (Some(1..), Some(_)) | (Some(0), Some(28..)))
}

fn sixel(attributes: Option<&[Option<u32>]>) -> bool {
    matches!(attributes, Some([_, capabilities @ ..]) if capabilities.contains(&Some(4)))
}

/// The output path selected for an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageProtocol {
    /// Ordinary Kitty placeholder cells.
    KittyPlaceholders,
    /// Native Kitty placements.
    Kitty,
    /// iTerm2 inline images.
    Iterm2,
    /// Sixel raster images.
    Sixel,
    /// Ordinary character cells.
    Symbols,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Policy {
    profile: Profile,
    width: WidthMode,
    eaw_wide: bool,
}

impl Policy {
    fn of<W: Write>(screen: &Screen<W>) -> Self {
        Self {
            profile: screen.color_profile(),
            width: screen.width_mode(),
            eaw_wide: screen.eaw_wide(),
        }
    }
}

#[derive(Debug)]
struct CachedImage {
    source: Image,
    options: ImageOptions,
    symbols: SymbolSet,
    policy: Policy,
    pixels: Option<Size>,
    protocol: ImageProtocol,
    size: Size,
    placement: PlacementOptions,
}

/// Prepare and stage images for one screen and terminal session.
///
/// Use [`draw`](Self::draw) for images, then write ordinary text to the screen.
/// Finish frames with [`Screen::render`]. The screen owns placement geometry,
/// resource uploads, deletion, and output recovery. This value caches encoded
/// data; dropping it performs no terminal I/O.
///
/// IDs share the screen's placement namespace. Reserve each managed ID until
/// removal. Keep this renderer paired with the same screen. Repaint underlying
/// content after image movement or removal. Cell removal clears cells that
/// still match the last draw to default-style blanks; changed overlay cells remain.
///
/// Native Kitty uses protocol z-order. For text cutouts, use the explicit
/// [`PreparedImage::crop`](crate::PreparedImage::crop) API. Placeholder and symbol cells support ordinary
/// text overlays.
///
/// ```
/// use uncurses::{layout::Rect, screen::Screen};
/// use uncurses_images::{Image, ImageCapabilities, ImageOptions, ImageRenderer};
///
/// let source = Image::from_rgb(1, 1, vec![255, 0, 0])?;
/// let mut screen = Screen::new(Vec::new(), (20, 6));
/// let mut images = ImageRenderer::new(ImageCapabilities::default());
/// images.draw(&mut screen, 1, Rect::new(2, 1, 8, 3), &source, &ImageOptions::default())?;
/// screen.render()?;
/// images.remove(&mut screen, 1);
/// screen.render()?;
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct ImageRenderer {
    /// Support and physical geometry used by subsequent draws.
    pub capabilities: ImageCapabilities,
    /// Glyph families used by subsequent symbol preparations.
    pub symbols: SymbolSet,
    tmux_passthrough: bool,
    cache: BTreeMap<PlacementId, CachedImage>,
}

impl ImageRenderer {
    /// Configure explicit capabilities. Unknown support selects symbols.
    ///
    /// Resources use direct output. Use [`from_program`](Self::from_program)
    /// for automatic tmux forwarding.
    pub fn new(capabilities: ImageCapabilities) -> Self {
        Self {
            capabilities,
            symbols: SymbolSet::DEFAULT,
            tmux_passthrough: false,
            cache: BTreeMap::new(),
        }
    }

    /// Configure from the program's current reports and environment.
    ///
    /// In tmux, wrap placeholder uploads, virtual placements, and deletion.
    /// Ordinary cells stay in the pane's text grid. Enable `allow-passthrough`
    /// in tmux. Other multiplexer transports require explicit forwarding.
    pub fn from_program<I: Input, W: Write>(program: &Program<I, W>) -> Self {
        let mut renderer = Self::new(ImageCapabilities::from_program(program));
        renderer.tmux_passthrough = program
            .env()
            .get("TMUX")
            .is_some_and(|value| !value.is_empty());
        renderer
    }

    fn select<W: Write>(&self, screen: &Screen<W>, area: Rect) -> ImageProtocol {
        let caps = self.capabilities;
        if caps.pixels().is_some() {
            if caps.kitty_placeholders
                && screen.color_profile() == Profile::TrueColor
                && area.width <= 297
                && area.height <= 297
            {
                return ImageProtocol::KittyPlaceholders;
            }
            if caps.kitty {
                return ImageProtocol::Kitty;
            }
            if area.bottom() < screen.size().height {
                if caps.iterm2 {
                    return ImageProtocol::Iterm2;
                }
                if caps.sixel {
                    return ImageProtocol::Sixel;
                }
            }
        }
        ImageProtocol::Symbols
    }

    /// Prepare and stage one image without terminal I/O.
    ///
    /// Selection prefers placeholders, Kitty, iTerm2, Sixel, then symbols.
    /// Graphics require valid physical cell dimensions. Placeholders also
    /// require true color and dimensions at most 297 cells. Raster protocols
    /// require one spare row below the image.
    ///
    /// The nonzero ID and nonempty rectangle must fit the screen. Preparation
    /// errors leave the desired frame unchanged. Repeated draws reuse encoded
    /// data when the source, size, options, and output policy match.
    /// Source clones share this identity. Caller options remain unchanged.
    pub fn draw<W: Write>(
        &mut self,
        screen: &mut Screen<W>,
        id: PlacementId,
        area: Rect,
        source: &Image,
        options: &ImageOptions,
    ) -> io::Result<ImageProtocol> {
        if id == 0 || !fits(area, screen.size()) {
            return Err(invalid(
                "image ID must be nonzero and its rectangle must fit the screen",
            ));
        }
        let protocol = self.select(screen, area);
        let policy = Policy::of(screen);
        let pixels = self.capabilities.pixels();
        let size = Size::new(area.width, area.height);
        let reuse = self.cache.get(&id).is_some_and(|entry| {
            entry.source.same_source(source)
                && entry.options == *options
                && entry.size == size
                && entry.policy == policy
                && entry.pixels == pixels
                && entry.protocol == protocol
                && entry.symbols == self.symbols
        });
        let replacement = if reuse {
            None
        } else {
            let placement = match protocol {
                ImageProtocol::KittyPlaceholders => {
                    let image = source.kitty_placeholders(size, pixels.unwrap(), options)?;
                    let mut placement = image.placement(screen)?;
                    if self.tmux_passthrough {
                        placement.upload =
                            crate::protocols::kitty::wrap_tmux(&placement.upload)?.into();
                        placement.delete =
                            crate::protocols::kitty::wrap_tmux(&placement.delete)?.into();
                    }
                    placement
                }
                ImageProtocol::Symbols => {
                    let mut sampling = options.clone();
                    if let Some(pixels) = pixels {
                        sampling.cell_pixels = (pixels.width, pixels.height);
                    }
                    PlacementOptions {
                        cells: Some(Arc::new(source.symbols(
                            size,
                            &sampling,
                            self.symbols.clone(),
                            screen,
                        )?)),
                        ..Default::default()
                    }
                }
                native => {
                    let backend = match native {
                        ImageProtocol::Kitty => Backend::Kitty,
                        ImageProtocol::Iterm2 => Backend::Iterm2,
                        ImageProtocol::Sixel => Backend::Sixel,
                        _ => unreachable!(),
                    };
                    let image = source.prepare(backend, size, pixels.unwrap(), options)?;
                    image.placement(1)?
                }
            };
            Some(CachedImage {
                source: source.clone(),
                options: options.clone(),
                symbols: self.symbols.clone(),
                policy,
                pixels,
                protocol,
                size,
                placement,
            })
        };
        let entry = replacement
            .as_ref()
            .or_else(|| self.cache.get(&id))
            .unwrap();
        screen.set_placement(id, area, &entry.placement)?;
        if let Some(entry) = replacement {
            self.cache.insert(id, entry);
        }
        Ok(protocol)
    }

    /// Stage removal. The next render releases the image's terminal resource.
    pub fn remove<W: Write>(&mut self, screen: &mut Screen<W>, id: PlacementId) {
        screen.remove_placement(id);
        self.cache.remove(&id);
    }
}

fn fits(area: Rect, size: Size) -> bool {
    area.width > 0
        && area.height > 0
        && u32::from(area.x) + u32::from(area.width) <= u32::from(size.width)
        && u32::from(area.y) + u32::from(area.height) <= u32::from(size.height)
}

#[cfg(test)]
mod tests;
