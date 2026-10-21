//! Kitty image resources and native placements.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io;
use std::sync::{
    Arc, LazyLock, Weak,
    atomic::{AtomicU32, Ordering},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use uncurses::ansi::graphics;
use uncurses::layout::{Position, Rect, Size};
use uncurses::screen::{PreparedRegion, RegionOp, RegionResource};

use crate::{Image, ImageOptions};

static NEXT_ID: LazyLock<AtomicU32> = LazyLock::new(|| {
    let seed = RandomState::new().hash_one(std::process::id()) as u32;
    AtomicU32::new((seed & 0x7fff_ffff) + 2)
});

pub(crate) fn next_id() -> io::Result<u32> {
    NEXT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| io::Error::other("Kitty graphics identifiers exhausted"))
}

#[derive(Debug)]
pub(crate) struct CachedResource {
    size: Size,
    options: ImageOptions,
    id: u32,
    resource: Weak<RegionResource>,
    virtual_resource: Weak<RegionResource>,
    pinned_resource: Weak<RegionResource>,
}

pub(crate) fn image_resource(
    image: &Image,
    size: Size,
    options: &ImageOptions,
) -> io::Result<(u32, Arc<RegionResource>)> {
    let mut cache = image
        .cache
        .lock()
        .map_err(|error| io::Error::other(error.to_string()))?;
    cache.retain(|entry| entry.resource.strong_count() != 0);
    for entry in cache.iter() {
        if entry.size == size
            && entry.options.same_raster(options)
            && let Some(resource) = entry.resource.upgrade()
        {
            return Ok((entry.id, resource));
        }
    }
    let pixels = image.rasterize(size, options)?;
    let id = next_id()?;
    let encoded = STANDARD.encode(pixels.as_raw());
    let chunks = encoded.as_bytes().chunks(4096);
    let count = chunks.len();
    let mut create = Vec::new();
    for (index, chunk) in chunks.enumerate() {
        let more = usize::from(index + 1 < count);
        let header = if index == 0 {
            format!(
                "a=t,t=d,f=24,s={},v={},i={id},q=2,m={more}",
                pixels.width(),
                pixels.height()
            )
        } else {
            format!("m={more},q=2")
        };
        graphics::write_kitty_graphics(&mut create, &[&header], chunk)?;
    }
    let resource = Arc::new(RegionResource {
        create,
        release: command(&format!("a=d,d=I,i={id},q=2"), &[])?,
        ..Default::default()
    });
    cache.push(CachedResource {
        size,
        options: options.clone(),
        id,
        resource: Arc::downgrade(&resource),
        virtual_resource: Weak::new(),
        pinned_resource: Weak::new(),
    });
    Ok((id, resource))
}

pub(crate) fn unicode_resource(
    image: &Image,
    size: Size,
    options: &ImageOptions,
) -> io::Result<(u32, Arc<RegionResource>)> {
    virtual_resource(image, size, options, true)
}

pub(crate) fn pinned_resource(
    image: &Image,
    size: Size,
    options: &ImageOptions,
) -> io::Result<(u32, Arc<RegionResource>)> {
    virtual_resource(image, size, options, false)
}

fn virtual_resource(
    image: &Image,
    size: Size,
    options: &ImageOptions,
    placeholders: bool,
) -> io::Result<(u32, Arc<RegionResource>)> {
    let (id, parent) = image_resource(image, size, options)?;
    let mut cache = image
        .cache
        .lock()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let entry = cache
        .iter_mut()
        .find(|entry| entry.id == id)
        .expect("retained image resource");
    let cached = if placeholders {
        &mut entry.virtual_resource
    } else {
        &mut entry.pinned_resource
    };
    if let Some(resource) = cached.upgrade() {
        return Ok((id, resource));
    }
    // The identifier allocator reserves both endpoints for virtual placements.
    let placement = if placeholders { 1 } else { u32::MAX };
    let resource = Arc::new(RegionResource {
        dependencies: vec![parent],
        create: command(
            &format!(
                "a=p,U=1,i={id},p={placement},c={},r={},q=2",
                size.width, size.height
            ),
            &[],
        )?,
        release: command(&format!("a=d,d=i,i={id},p={placement},q=2"), &[])?,
        minimum_color_profile: placeholders.then_some(uncurses::color::Profile::TrueColor),
    });
    *cached = Arc::downgrade(&resource);
    Ok((id, resource))
}

pub(crate) fn command(options: &str, data: &[u8]) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    graphics::write_kitty_graphics(&mut bytes, &[options], data)?;
    Ok(bytes)
}

pub(crate) fn prepare_placement(
    image: &Image,
    size: Size,
    options: &ImageOptions,
    visible: Vec<Rect>,
) -> io::Result<PreparedRegion> {
    let mut prepared = PreparedRegion::default();
    let (id, resource) = image_resource(image, size, options)?;
    prepared.draw.push(RegionOp::Resource(resource));
    for rect in visible {
        let placement = next_id()?;
        let (cw, ch) = options.cell_pixels;
        let options = format!(
            "a=p,i={id},p={placement},x={},y={},w={},h={},c={},r={},C=1,q=2",
            u32::from(rect.x) * u32::from(cw),
            u32::from(rect.y) * u32::from(ch),
            u32::from(rect.width) * u32::from(cw),
            u32::from(rect.height) * u32::from(ch),
            rect.width,
            rect.height,
        );
        let position = Position::new(rect.x, rect.y);
        prepared.draw.extend([
            RegionOp::MoveTo(position),
            RegionOp::Write(command(&options, &[])?),
            RegionOp::SetCursor(position),
        ]);
        prepared.erase.push(RegionOp::Command(command(
            &format!("a=d,d=i,i={id},p={placement},q=2"),
            &[],
        )?));
    }
    Ok(prepared)
}
