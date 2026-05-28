//! Subtree paint-space affine transforms.
//!
//! Used by [`crate::Window::with_paint_transform`] to scale and translate
//! every primitive emitted by a subtree (and every hitbox it inserts) into
//! screen space. Composes uniform-scale + translation only — no rotation, no
//! shear — which is sufficient for canvas zoom while remaining cheap to
//! compose, invert, and apply at scene-emit time.
//!
//! The transform is `T(p) = scale * p + translate`. To scale around a pivot
//! point, build with [`ScreenTransform::scale_around_pivot`]; the constructor
//! folds the pivot into the translate so applications stay one mul + one add.

use std::ops::Mul;

use crate::{Bounds, Pixels, Point, Size, px};

/// A 2D affine transform applied to paint and hit-test geometry. Restricted
/// to uniform scale + translation, so composition and inversion stay closed
/// in this representation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenTransform {
    /// Uniform scale factor. `1.0` is identity.
    pub scale: f32,
    /// Translation applied after scale, in screen-space logical pixels.
    pub translate: Point<Pixels>,
}

impl Default for ScreenTransform {
    fn default() -> Self {
        Self::identity()
    }
}

impl ScreenTransform {
    /// Identity: `T(p) = p`.
    pub const fn identity() -> Self {
        Self {
            scale: 1.0,
            translate: Point::new(px(0.0), px(0.0)),
        }
    }

    /// Pure translation.
    pub const fn translate(offset: Point<Pixels>) -> Self {
        Self {
            scale: 1.0,
            translate: offset,
        }
    }

    /// Uniform scale anchored at `pivot`: points at the pivot stay put, points
    /// further from the pivot move further from it by `scale`. Equivalent to
    /// `translate(pivot) ∘ scale(scale) ∘ translate(-pivot)` collapsed into a
    /// single affine.
    pub fn scale_around_pivot(scale: f32, pivot: Point<Pixels>) -> Self {
        let factor = 1.0 - scale;
        Self {
            scale,
            translate: Point::new(pivot.x * factor, pivot.y * factor),
        }
    }

    /// True if this transform makes no geometric change.
    pub fn is_identity(self) -> bool {
        self.scale == 1.0 && self.translate.x.0 == 0.0 && self.translate.y.0 == 0.0
    }

    /// Compose two transforms: `outer ∘ inner`, i.e. apply `inner` first then
    /// `outer`. For uniform-scale + translation this stays in the same form:
    /// `(s_o * s_i, s_o * t_i + t_o)`.
    pub fn compose(outer: ScreenTransform, inner: ScreenTransform) -> Self {
        Self {
            scale: outer.scale * inner.scale,
            translate: Point::new(
                outer.scale * inner.translate.x + outer.translate.x,
                outer.scale * inner.translate.y + outer.translate.y,
            ),
        }
    }

    /// Inverse transform: maps screen-space points back into the subtree's
    /// pre-transform space. Returns the identity if `scale == 0`, which never
    /// occurs for finite zoom factors but guards against division by zero.
    pub fn inverse(self) -> Self {
        if self.scale == 0.0 {
            return Self::identity();
        }
        let inv_scale = 1.0 / self.scale;
        Self {
            scale: inv_scale,
            translate: Point::new(-self.translate.x * inv_scale, -self.translate.y * inv_scale),
        }
    }

    /// Apply to a single point.
    pub fn apply_point(self, p: Point<Pixels>) -> Point<Pixels> {
        Point::new(p.x * self.scale + self.translate.x, p.y * self.scale + self.translate.y)
    }

    /// Apply uniform scale to a size (translation has no effect on extent).
    pub fn apply_size(self, s: Size<Pixels>) -> Size<Pixels> {
        Size {
            width: s.width * self.scale,
            height: s.height * self.scale,
        }
    }

    /// Apply to a rectangle. Origin transforms as a point; size scales.
    pub fn apply_bounds(self, b: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds {
            origin: self.apply_point(b.origin),
            size: self.apply_size(b.size),
        }
    }
}

impl Mul<ScreenTransform> for ScreenTransform {
    type Output = ScreenTransform;

    /// `a * b` composes `a` after `b`: `(a * b).apply(p) == a.apply(b.apply(p))`.
    fn mul(self, rhs: ScreenTransform) -> Self::Output {
        ScreenTransform::compose(self, rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::point;

    fn approx(a: Point<Pixels>, b: Point<Pixels>) -> bool {
        (a.x.0 - b.x.0).abs() < 1e-4 && (a.y.0 - b.y.0).abs() < 1e-4
    }

    #[test]
    fn identity_is_no_op() {
        let p = point(px(10.0), px(20.0));
        assert_eq!(ScreenTransform::identity().apply_point(p), p);
    }

    #[test]
    fn pivot_point_is_fixed() {
        let pivot = point(px(100.0), px(50.0));
        let t = ScreenTransform::scale_around_pivot(2.5, pivot);
        assert!(approx(t.apply_point(pivot), pivot));
    }

    #[test]
    fn compose_is_associative_with_apply() {
        let a = ScreenTransform::scale_around_pivot(1.5, point(px(10.0), px(20.0)));
        let b = ScreenTransform::translate(point(px(7.0), px(-3.0)));
        let p = point(px(50.0), px(60.0));
        let composed = (a * b).apply_point(p);
        let stepwise = a.apply_point(b.apply_point(p));
        assert!(approx(composed, stepwise));
    }

    #[test]
    fn inverse_round_trips() {
        let t = ScreenTransform::scale_around_pivot(2.0, point(px(40.0), px(80.0)))
            * ScreenTransform::translate(point(px(5.0), px(7.0)));
        let p = point(px(123.0), px(45.0));
        let round_trip = t.inverse().apply_point(t.apply_point(p));
        assert!(approx(round_trip, p));
    }
}
