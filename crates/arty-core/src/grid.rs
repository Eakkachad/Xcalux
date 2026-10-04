//! Sparse tile grid: only tiles that were ever painted exist.
//!
//! The coordinate map itself is `Arc`-shared, so cloning a grid (document
//! snapshot, structure undo) is O(1). The first write after a clone copies
//! the map once; tiles stay shared until each one is written.

use std::sync::Arc;

use ahash::AHashMap;

use crate::tile::{TileCoord, TilePixels, TileRef, new_tile};

#[derive(Clone, Default)]
pub struct TileGrid {
    tiles: Arc<AHashMap<TileCoord, TileRef>>,
}

impl TileGrid {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(n: usize) -> Self {
        Self { tiles: Arc::new(AHashMap::with_capacity(n)) }
    }

    /// True when both grids share one map, i.e. neither was written since
    /// one was cloned from the other.
    #[inline]
    pub fn shares_storage(&self, other: &TileGrid) -> bool {
        Arc::ptr_eq(&self.tiles, &other.tiles)
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
        Arc::make_mut(Arc::make_mut(&mut self.tiles).entry(c).or_insert_with(new_tile))
    }

    /// Insert a tile, returning the previous one.
    pub fn insert(&mut self, c: TileCoord, tile: TileRef) -> Option<TileRef> {
        Arc::make_mut(&mut self.tiles).insert(c, tile)
    }

    /// Replace a tile (or remove it with `None`), returning the previous one.
    pub fn replace(&mut self, c: TileCoord, tile: Option<TileRef>) -> Option<TileRef> {
        match tile {
            Some(t) => self.insert(c, t),
            // Removing a missing tile must not un-share the map.
            None if !self.tiles.contains_key(&c) => None,
            None => Arc::make_mut(&mut self.tiles).remove(&c),
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
        match Arc::get_mut(&mut self.tiles) {
            Some(tiles) => tiles.clear(),
            None => self.tiles = Arc::default(),
        }
    }

    /// Bytes of pixel data uniquely or jointly owned by this grid.
    pub fn pixel_bytes(&self) -> usize {
        self.tiles.len() * std::mem::size_of::<TilePixels>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_write_after_clone_copies_the_map_only() {
        let (a, b) = (TileCoord::new(0, 0), TileCoord::new(1, 0));
        let mut grid = TileGrid::new();
        grid.get_mut_or_create(a)[0][0] = [1; 4];
        grid.get_mut_or_create(b)[0][0] = [2; 4];
        let snap = grid.clone();
        assert!(grid.shares_storage(&snap));

        grid.get_mut_or_create(a)[0][0] = [3; 4];
        grid.replace(TileCoord::new(5, 5), None);
        assert!(!grid.shares_storage(&snap), "write un-shares the map");
        assert_eq!(snap.get(a).unwrap()[0][0], [1; 4], "snapshot keeps old pixels");
        assert_eq!(grid.get(a).unwrap()[0][0], [3; 4]);
        assert!(Arc::ptr_eq(grid.get_ref(b).unwrap(), snap.get_ref(b).unwrap()), "untouched tile still shared");

        grid.insert(TileCoord::new(2, 0), new_tile());
        grid.clear();
        assert!(grid.is_empty());
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn removing_a_missing_tile_keeps_sharing() {
        let mut grid = TileGrid::new();
        grid.get_mut_or_create(TileCoord::new(0, 0));
        let snap = grid.clone();
        assert!(grid.replace(TileCoord::new(9, 9), None).is_none());
        assert!(grid.shares_storage(&snap));
    }
}
