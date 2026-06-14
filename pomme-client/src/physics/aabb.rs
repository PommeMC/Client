use glam::{DVec3, dvec3};

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub fn new(min: DVec3, max: DVec3) -> Self {
        Self { min, max }
    }

    /// Unit cube occupying the block at the given coordinates.
    pub fn block(x: i32, y: i32, z: i32) -> Self {
        Self::new(
            dvec3(x as f64, y as f64, z as f64),
            dvec3((x + 1) as f64, (y + 1) as f64, (z + 1) as f64),
        )
    }

    pub fn from_center(center: DVec3, half_width: f64, half_height: f64) -> Self {
        Self {
            min: dvec3(center.x - half_width, center.y, center.z - half_width),
            max: dvec3(
                center.x + half_width,
                center.y + half_height * 2.0,
                center.z + half_width,
            ),
        }
    }

    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
            && self.min.z < other.max.z
            && self.max.z > other.min.z
    }

    pub fn offset(self, offset: DVec3) -> Self {
        Self {
            min: self.min + offset,
            max: self.max + offset,
        }
    }

    pub fn deflate(self, amount: f64) -> Self {
        Self {
            min: self.min + amount,
            max: self.max - amount,
        }
    }

    pub fn expand(self, delta: DVec3) -> Self {
        let mut min = self.min;
        let mut max = self.max;

        if delta.x < 0.0 {
            min.x += delta.x;
        } else {
            max.x += delta.x;
        }
        if delta.y < 0.0 {
            min.y += delta.y;
        } else {
            max.y += delta.y;
        }
        if delta.z < 0.0 {
            min.z += delta.z;
        } else {
            max.z += delta.z;
        }

        Self { min, max }
    }

    pub fn clip_x_collide(&self, other: &Aabb, dx: f64) -> f64 {
        self.clip_axis(other, dx, Axis::X)
    }

    pub fn clip_y_collide(&self, other: &Aabb, dy: f64) -> f64 {
        self.clip_axis(other, dy, Axis::Y)
    }

    pub fn clip_z_collide(&self, other: &Aabb, dz: f64) -> f64 {
        self.clip_axis(other, dz, Axis::Z)
    }

    fn clip_axis(&self, other: &Aabb, mut delta: f64, axis: Axis) -> f64 {
        let (c1, c2) = axis.cross_axes();

        if component(other.max, c1) <= component(self.min, c1)
            || component(other.min, c1) >= component(self.max, c1)
        {
            return delta;
        }
        if component(other.max, c2) <= component(self.min, c2)
            || component(other.min, c2) >= component(self.max, c2)
        {
            return delta;
        }

        if delta > 0.0 && component(other.max, axis) <= component(self.min, axis) {
            let clip = component(self.min, axis) - component(other.max, axis);
            if clip < delta {
                delta = clip;
            }
        } else if delta < 0.0 && component(other.min, axis) >= component(self.max, axis) {
            let clip = component(self.max, axis) - component(other.min, axis);
            if clip > delta {
                delta = clip;
            }
        }

        delta
    }
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn cross_axes(self) -> (Axis, Axis) {
        match self {
            Axis::X => (Axis::Y, Axis::Z),
            Axis::Y => (Axis::X, Axis::Z),
            Axis::Z => (Axis::X, Axis::Y),
        }
    }
}

fn component(v: DVec3, axis: Axis) -> f64 {
    match axis {
        Axis::X => v.x,
        Axis::Y => v.y,
        Axis::Z => v.z,
    }
}
