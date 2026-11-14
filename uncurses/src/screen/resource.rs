use std::sync::{Arc, Weak};

/// An immutable terminal resource shared by screen placements.
///
/// Clones identify the same resource. Each new handle owns independent terminal
/// resources, so its commands must use distinct protocol identifiers. Prepare
/// a new resource to replace its content.
/// Share a handle within one screen and terminal session.
///
/// The screen uploads the resource before its first frame and deletes it after
/// its final managed placement disappears. Committed output keeps the terminal
/// resource alive without retaining the upload buffer.
#[derive(Clone, Debug)]
pub struct PlacementResource(Arc<ResourceData>);

#[derive(Debug)]
struct ResourceData {
    upload: Arc<[u8]>,
    delete: Arc<[u8]>,
}

impl PlacementResource {
    /// Create a resource with complete upload and deletion commands.
    ///
    /// Both command streams must preserve the cursor, pen, and terminal modes.
    /// Upload must be safe to repeat after terminal invalidation.
    pub fn new(upload: impl Into<Arc<[u8]>>, delete: impl Into<Arc<[u8]>>) -> Self {
        Self(Arc::new(ResourceData {
            upload: upload.into(),
            delete: delete.into(),
        }))
    }

    /// The upload commands, for explicit protocol output.
    pub fn upload(&self) -> &[u8] {
        &self.0.upload
    }

    /// The deletion commands, for explicit protocol output.
    pub fn delete(&self) -> &[u8] {
        &self.0.delete
    }

    pub(crate) fn key(&self) -> ResourceKey {
        ResourceKey(Arc::downgrade(&self.0))
    }
}

impl PartialEq for PlacementResource {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for PlacementResource {}

#[derive(Clone)]
pub(crate) struct ResourceKey(Weak<ResourceData>);

impl PartialEq for ResourceKey {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ResourceKey {}
