use winit::dpi::{PhysicalPosition, PhysicalSize, Pixel};

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Default, Hash)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

impl Position {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

impl<P: Pixel> From<PhysicalPosition<P>> for Position {
    fn from(value: PhysicalPosition<P>) -> Self {
        Self {
            x: value.x.cast(),
            y: value.y.cast(),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Default, Hash)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

impl Size {
    pub const fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }
}

impl<P: Pixel> From<PhysicalSize<P>> for Size {
    fn from(value: PhysicalSize<P>) -> Self {
        Self {
            width: value.width.cast(),
            height: value.height.cast(),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
pub struct Color {
    r: u8,
    g: u8,
    b: u8,
    a: f32,
}

impl Color {
    pub const WHITE: Self = Self::new(255, 255, 255, 1.0);
    pub const BLACK: Self = Self::new(0, 0, 0, 1.0);

    pub const fn new(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { r, g, b, a }
    }
}

impl From<Color> for [f32; 4] {
    fn from(value: Color) -> Self {
        [
            value.r as f32 / 255.0,
            value.g as f32 / 255.0,
            value.b as f32 / 255.0,
            value.a,
        ]
    }
}
