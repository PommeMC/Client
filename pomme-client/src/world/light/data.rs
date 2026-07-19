//! Vanilla `DataLayer`: one section's light as 4096 nibbles, lazily
//! materialized so homogeneous layers (all-0, all-15) stay allocation-free.

pub(crate) const LAYER_BYTES: usize = 2048;

#[derive(Clone)]
pub(crate) struct DataLayer {
    data: Option<Box<[u8; LAYER_BYTES]>>,
    default: u8,
}

impl Default for DataLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl DataLayer {
    pub fn new() -> Self {
        Self::filled(0)
    }

    pub fn filled(default: u8) -> Self {
        Self {
            data: None,
            default,
        }
    }

    pub fn from_bytes(data: Box<[u8; LAYER_BYTES]>) -> Self {
        Self {
            data: Some(data),
            default: 0,
        }
    }

    /// `y<<8 | z<<4 | x`; even indices take the low nibble.
    fn index(x: i32, y: i32, z: i32) -> usize {
        debug_assert!(
            (0..16).contains(&x) && (0..16).contains(&y) && (0..16).contains(&z),
            "DataLayer coords out of range: {x} {y} {z}"
        );
        (y as usize) << 8 | (z as usize) << 4 | x as usize
    }

    pub fn get(&self, x: i32, y: i32, z: i32) -> u8 {
        let Some(data) = &self.data else {
            return self.default;
        };
        let index = Self::index(x, y, z);
        data[index >> 1] >> (4 * (index & 1)) & 0xF
    }

    pub fn set(&mut self, x: i32, y: i32, z: i32, value: u8) {
        let index = Self::index(x, y, z);
        let data = self.materialize();
        let shift = 4 * (index & 1);
        data[index >> 1] = data[index >> 1] & !(0xF << shift) | (value & 0xF) << shift;
    }

    pub fn fill(&mut self, value: u8) {
        self.default = value;
        self.data = None;
    }

    /// Vanilla `isEmpty`: homogeneous zero.
    pub fn is_empty(&self) -> bool {
        self.data.is_none() && self.default == 0
    }

    /// Vanilla `isDefinitelyHomogenous`: still a lazy fill, no byte array.
    pub fn is_homogeneous(&self) -> bool {
        self.data.is_none()
    }

    /// A full layer of the homogeneous default, both nibbles packed per byte.
    fn default_bytes(&self) -> Box<[u8; LAYER_BYTES]> {
        let nibble = self.default & 0xF;
        Box::new([nibble | nibble << 4; LAYER_BYTES])
    }

    fn materialize(&mut self) -> &mut [u8; LAYER_BYTES] {
        if self.data.is_none() {
            self.data = Some(self.default_bytes());
        }
        self.data.as_mut().unwrap()
    }

    /// The layer as raw nibble bytes, materializing homogeneous layers.
    pub fn to_bytes(&self) -> Box<[u8; LAYER_BYTES]> {
        match &self.data {
            Some(data) => data.clone(),
            None => self.default_bytes(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nibble_layout_matches_vanilla() {
        let mut layer = DataLayer::new();
        assert!(layer.is_empty());
        layer.set(1, 2, 3, 15);
        // index = 2<<8 | 3<<4 | 1 = 561 (odd -> high nibble of byte 280).
        let bytes = layer.to_bytes();
        assert_eq!(bytes[280], 0xF0);
        assert_eq!(layer.get(1, 2, 3), 15);
        assert_eq!(layer.get(0, 2, 3), 0);
        layer.set(0, 2, 3, 7);
        assert_eq!(layer.get(0, 2, 3), 7);
        assert_eq!(layer.get(1, 2, 3), 15);
        assert!(!layer.is_empty());
    }

    #[test]
    fn homogeneous_fill() {
        let mut layer = DataLayer::filled(15);
        assert_eq!(layer.get(0, 0, 0), 15);
        assert!(!layer.is_empty());
        assert_eq!(layer.to_bytes()[0], 0xFF);
        layer.set(5, 5, 5, 3);
        assert_eq!(layer.get(5, 5, 5), 3);
        assert_eq!(layer.get(6, 5, 5), 15);
        layer.fill(0);
        assert!(layer.is_empty());
        assert_eq!(layer.get(5, 5, 5), 0);
    }
}
