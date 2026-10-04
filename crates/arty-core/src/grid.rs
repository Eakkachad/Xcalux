//! Sparse tile grid: only tiles that were ever painted exist.

use std::sync::Arc;

use ahash::AHashMap;

use crate::tile::{TileCoord, TilePixels, TileRef, new_tile};

#[derive(Clone, Default)]
pub struct TileGrid {
    tiles: AHashMap<TileCoord, TileRef>,
}

impl TileGrid {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn get(&self, c: TileCoord) -> Option<&TilePixels> {
        self.tiles.get(&c).map(|t| &**t)
    }

    #[inline]
    pub fn get_ref(&self, c: TileCoord) -> Option<&TileRef> {
        self.tiles.get(&c)
    }

    /// Mutable access, creating a transparent tile if missing and
    /// un-sharing it if a snapshot still holds the old pixels.
    #[inline]
    pub fn get_mut_or_create(&mut self, c: TileCoord) -> &mut TilePixels {
        Arc::make_mut(self.tiles.entry(c).or_insert_with(new_tile))
    }

    /// Replace a tile (or remove it with `None`), returning the previous one.
    pub fn replace(&mut self, c: TileCoord, tile: Option<TileRef>) -> Option<TileRef> {
        match tile {
            Some(t) => self.tiles.insert(c, t),
            None => self.tiles.remove(&c),
        }
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    pub fn coords(&self) -> impl Iterator<Item = TileCoord> + '_ {
        self.tiles.keys().copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (TileCoord, &TileRef)> {
        self.tiles.iter().map(|(c, t)| (*c, t))
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    /// Bytes of pixel data uniquely or jointly owned by this grid.
    pub fn pixel_bytes(&self) -> usize {
        self.tiles.len() * std::mem::size_of::<TilePixels>()
    }
}
