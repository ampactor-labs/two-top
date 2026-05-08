#![deny(clippy::disallowed_types)]

use core::ops::{Add, Mul, Sub};

pub type Fix = fixed::types::I16F16;
pub type FixWide = fixed::types::I32F32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Vec2F {
    pub x: Fix,
    pub y: Fix,
}

impl Add for Vec2F {
    type Output = Vec2F;
    fn add(self, rhs: Vec2F) -> Vec2F {
        Vec2F { x: self.x + rhs.x, y: self.y + rhs.y }
    }
}

impl Sub for Vec2F {
    type Output = Vec2F;
    fn sub(self, rhs: Vec2F) -> Vec2F {
        Vec2F { x: self.x - rhs.x, y: self.y - rhs.y }
    }
}

impl Mul<Fix> for Vec2F {
    type Output = Vec2F;
    fn mul(self, rhs: Fix) -> Vec2F {
        Vec2F { x: self.x * rhs, y: self.y * rhs }
    }
}

impl Vec2F {
    /// Squared magnitude as `Fix`, **saturating** at `Fix::MAX` (~32767).
    ///
    /// Use for radius-scale comparisons (collision, hit-radii) where both
    /// sides are guaranteed small. For arena-scale distance ranking
    /// (e.g. "which player is closer to the pickup?") use
    /// [`Vec2F::length_sq_wide`] — this method silently saturates above
    /// vector magnitudes of ~181 cm.
    pub fn length_sq(self) -> Fix {
        Fix::saturating_from_num(self.length_sq_wide())
    }

    /// Squared magnitude as `FixWide`. Does not saturate within any
    /// reasonable arena range. Use for arena-scale distance comparisons
    /// or whenever inputs may exceed ~181 cm.
    pub fn length_sq_wide(self) -> FixWide {
        let xx: FixWide = self.x.wide_mul(self.x);
        let yy: FixWide = self.y.wide_mul(self.y);
        xx + yy
    }

    /// Magnitude as `Fix`. Internally widens through `FixWide` because
    /// `cordic::sqrt` squares candidates during its iterative
    /// approximation and overflows `Fix` near `Fix::MAX` even when the
    /// mathematical result fits.
    pub fn length(self) -> Fix {
        Fix::saturating_from_num(cordic::sqrt(self.length_sq_wide()))
    }

    pub fn dot(self, other: Vec2F) -> Fix {
        let xx: FixWide = self.x.wide_mul(other.x);
        let yy: FixWide = self.y.wide_mul(other.y);
        Fix::saturating_from_num(xx + yy)
    }

    pub fn cross(self, other: Vec2F) -> Fix {
        let xy: FixWide = self.x.wide_mul(other.y);
        let yx: FixWide = self.y.wide_mul(other.x);
        Fix::saturating_from_num(xy - yx)
    }

    pub fn normalize(self) -> Vec2F {
        let len = self.length();
        if len == Fix::ZERO {
            Vec2F::ZERO
        } else {
            Vec2F { x: self.x / len, y: self.y / len }
        }
    }

    pub fn rotate(self, radians: Fix) -> Vec2F {
        let (s, c) = sin_cos(radians);
        let cx: FixWide = c.wide_mul(self.x);
        let sy: FixWide = s.wide_mul(self.y);
        let sx: FixWide = s.wide_mul(self.x);
        let cy: FixWide = c.wide_mul(self.y);
        Vec2F {
            x: Fix::saturating_from_num(cx - sy),
            y: Fix::saturating_from_num(sx + cy),
        }
    }

    pub fn angle(self) -> Fix {
        atan2(self.y, self.x)
    }

    #[allow(clippy::disallowed_types)]
    pub fn to_f32(self) -> (f32, f32) {
        (self.x.to_num::<f32>(), self.y.to_num::<f32>())
    }
}

pub fn sqrt(x: Fix) -> Fix {
    // Widen before calling cordic so its internal squarings have headroom.
    let wide = FixWide::from_num(x);
    Fix::saturating_from_num(cordic::sqrt(wide))
}

pub const PI: Fix = Fix::lit("3.14159265358979323846");
pub const TWO_PI: Fix = Fix::lit("6.28318530717958647693");
pub const HALF_PI: Fix = Fix::lit("1.57079632679489661923");

fn normalize_angle(x: Fix) -> Fix {
    let mut n = x % TWO_PI;
    if n > PI {
        n -= TWO_PI;
    } else if n < -PI {
        n += TWO_PI;
    }
    n
}

pub fn sin(x: Fix) -> Fix {
    cordic::sin(normalize_angle(x))
}

pub fn cos(x: Fix) -> Fix {
    cordic::cos(normalize_angle(x))
}

pub fn sin_cos(x: Fix) -> (Fix, Fix) {
    let n = normalize_angle(x);
    (cordic::sin(n), cordic::cos(n))
}

pub fn atan2(y: Fix, x: Fix) -> Fix {
    cordic::atan2(y, x)
}

impl Vec2F {
    pub const ZERO: Vec2F = Vec2F {
        x: Fix::ZERO,
        y: Fix::ZERO,
    };

    pub const fn new(x: Fix, y: Fix) -> Self {
        Vec2F { x, y }
    }

    /// Construct from integer centimeter coordinates.
    ///
    /// Panics at compile or runtime if either component does not fit in
    /// `Fix`'s integer range (±32767 cm). Caller-side responsibility — our
    /// arenas are bounded to a few thousand cm, so this is unreachable in
    /// practice.
    pub const fn from_cm(x: i32, y: i32) -> Self {
        Vec2F {
            x: Fix::const_from_int(x),
            y: Fix::const_from_int(y),
        }
    }
}

/// Axis-aligned bounding box in fixed-point coords. Used for arena
/// walls and player collision shapes.
///
/// Stored as min/max corners — translation is just two `Vec2F`
/// additions, and the overlap predicate reads as four cheap integer
/// comparisons. Centroid math goes through halving constants (no
/// scalar division on `Vec2F` for now).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RectF {
    pub min: Vec2F,
    pub max: Vec2F,
}

impl RectF {
    pub const fn from_min_max(min: Vec2F, max: Vec2F) -> Self {
        Self { min, max }
    }

    /// Construct from a center and half-extents (half-width,
    /// half-height). Often more natural than corners when describing
    /// a player's collision shape ("32 cm half-extent square").
    pub fn from_center_half_extents(center: Vec2F, half: Vec2F) -> Self {
        Self {
            min: center - half,
            max: center + half,
        }
    }

    pub fn width(self) -> Fix {
        self.max.x - self.min.x
    }

    pub fn height(self) -> Fix {
        self.max.y - self.min.y
    }

    pub fn contains(self, p: Vec2F) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Strict overlap — touching edges (a.max == b.min) does not count.
    /// Strictness matters for collision resolution: "exactly resting
    /// against the wall" must be a non-overlap so the resolver doesn't
    /// keep nudging the player.
    pub fn overlaps(self, other: RectF) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
    }

    /// Translate the rect by `delta`.
    pub fn translated(self, delta: Vec2F) -> Self {
        Self {
            min: self.min + delta,
            max: self.max + delta,
        }
    }
}
