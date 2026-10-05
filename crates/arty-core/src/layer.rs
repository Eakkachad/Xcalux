//! Layers: raster pixel layers and folders.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::blend::BlendMode;
use crate::frame::Frame;
use crate::grid::TileGrid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LayerId(pub u32);

/// User-editable layer settings (everything except pixel content and tree
/// position), kept together so one history entry can restore them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerProps {
    pub name: String,
    pub visible: bool,
    /// 0..=1
    pub opacity: f32,
    pub blend: BlendMode,
    /// Clip to the nearest non-clipping layer below (CSP "Clip to layer below").
    pub clip: bool,
    /// Protect transparent pixels while painting.
    pub lock_alpha: bool,
    /// Disallow any edit.
    pub locked: bool,
    /// A reference layer for fills and the magic wand (CSP 参照レイヤー).
    #[serde(default)]
    pub reference: bool,
}

impl LayerProps {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            clip: false,
            lock_alpha: false,
            locked: false,
            reference: false,
        }
    }
}

#[derive(Clone)]
pub enum LayerContent {
    Raster(TileGrid),
    /// Children ordered bottom → top. `frame`: a frame border folder's
    /// panels, which mask the children (only folders can have one).
    Folder { children: Vec<LayerId>, expanded: bool, frame: Option<Arc<Frame>> },
}

#[derive(Clone)]
pub struct Layer {
    pub id: LayerId,
    pub props: LayerProps,
    pub content: LayerContent,
}

impl Layer {
    pub fn is_folder(&self) -> bool {
        matches!(self.content, LayerContent::Folder { .. })
    }

    pub fn raster(&self) -> Option<&TileGrid> {
        match &self.content {
            LayerContent::Raster(g) => Some(g),
            LayerContent::Folder { .. } => None,
        }
    }

    pub fn raster_mut(&mut self) -> Option<&mut TileGrid> {
        match &mut self.content {
            LayerContent::Raster(g) => Some(g),
            LayerContent::Folder { .. } => None,
        }
    }

    pub fn children(&self) -> Option<&[LayerId]> {
        match &self.content {
            LayerContent::Folder { children, .. } => Some(children),
            LayerContent::Raster(_) => None,
        }
    }
}
