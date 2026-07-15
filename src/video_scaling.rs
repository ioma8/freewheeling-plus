//! Coordinate and extent scaling for logical video layouts.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoScale {
    pub logical_width: i32,
    pub logical_height: i32,
    pub drawable_width: i32,
    pub drawable_height: i32,
    pub scale_x: f32,
    pub scale_y: f32,
}

/// Compute the drawable-to-logical scale, including the legacy dimension
/// fallbacks used by the C++ implementation.
pub fn compute_video_scale(
    logical_width: i32,
    logical_height: i32,
    drawable_width: i32,
    drawable_height: i32,
) -> VideoScale {
    let mut scale = VideoScale {
        logical_width,
        logical_height,
        drawable_width,
        drawable_height,
        scale_x: 1.0,
        scale_y: 1.0,
    };

    if scale.logical_width <= 0 {
        scale.logical_width = if scale.drawable_width > 0 {
            scale.drawable_width
        } else {
            1
        };
    }
    if scale.logical_height <= 0 {
        scale.logical_height = if scale.drawable_height > 0 {
            scale.drawable_height
        } else {
            1
        };
    }
    if scale.drawable_width <= 0 {
        scale.drawable_width = scale.logical_width;
    }
    if scale.drawable_height <= 0 {
        scale.drawable_height = scale.logical_height;
    }

    scale.scale_x = scale.drawable_width as f32 / scale.logical_width as f32;
    scale.scale_y = scale.drawable_height as f32 / scale.logical_height as f32;
    scale
}

/// Scale a positive extent using the C++ positive-half-up rounding rule.
pub fn scale_extent(value: i32, scale: f32) -> i32 {
    if value <= 0 {
        return 0;
    }
    if scale <= 0.0 {
        return value;
    }

    let scaled = (value as f32 * scale + 0.5) as i32;
    if scaled > 0 { scaled } else { 1 }
}

pub fn scale_font_point_size(point_size: i32, scale: f32) -> i32 {
    scale_extent(point_size, scale)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderMetrics {
    pub logical_width: i32,
    pub logical_height: i32,
    pub drawable_width: i32,
    pub drawable_height: i32,
    pub scale_x: f32,
    pub scale_y: f32,
}

impl RenderMetrics {
    pub fn from_drawable_size(
        logical_width: i32,
        logical_height: i32,
        drawable_width: i32,
        drawable_height: i32,
    ) -> Self {
        let scale = compute_video_scale(
            logical_width,
            logical_height,
            drawable_width,
            drawable_height,
        );
        Self {
            logical_width: scale.logical_width,
            logical_height: scale.logical_height,
            drawable_width: scale.drawable_width,
            drawable_height: scale.drawable_height,
            scale_x: scale.scale_x,
            scale_y: scale.scale_y,
        }
    }

    pub fn scale_x(&self, value: i32) -> i32 {
        scale_extent(value, self.scale_x)
    }
    pub fn scale_y(&self, value: i32) -> i32 {
        scale_extent(value, self.scale_y)
    }
    pub fn scale_font(&self, point_size: i32) -> i32 {
        scale_font_point_size(point_size, self.scale_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_scale_and_normalizes_dimensions() {
        let scale = compute_video_scale(0, -1, 800, 0);
        assert_eq!((scale.logical_width, scale.logical_height), (800, 1));
        assert_eq!((scale.drawable_width, scale.drawable_height), (800, 1));
        assert_eq!((scale.scale_x, scale.scale_y), (1.0, 1.0));
    }

    #[test]
    fn extent_preserves_legacy_edge_cases_and_rounding() {
        assert_eq!(scale_extent(0, 2.0), 0);
        assert_eq!(scale_extent(-3, 2.0), 0);
        assert_eq!(scale_extent(7, 0.0), 7);
        assert_eq!(scale_extent(7, -1.0), 7);
        assert_eq!(scale_extent(1, 0.4), 1);
        assert_eq!(scale_extent(3, 1.5), 5);
    }

    #[test]
    fn metrics_delegate_to_extent_scaling() {
        let metrics = RenderMetrics::from_drawable_size(640, 480, 1280, 960);
        assert_eq!(metrics.scale_x(5), 10);
        assert_eq!(metrics.scale_y(0), 0);
        assert_eq!(metrics.scale_font(9), 18);
    }

    #[test]
    fn all_production_metric_adapters_preserve_the_cpp_scaling_contract() {
        // `videoio`, the logical-display tree, and this compatibility module
        // all carry FweelinRenderMetrics at different API boundaries.  Keep
        // their C++ `FweelinComputeVideoScale`/`FweelinScaleExtent` behavior
        // bit-for-bit aligned for every dimension combination the SDL path
        // can supply (the C++ signed-negative cases are covered above).
        for (logical, drawable) in [
            ((0, 0), (0, 0)),
            ((0, 0), (1280, 960)),
            ((640, 480), (0, 0)),
            ((640, 480), (1280, 960)),
            ((640, 480), (1470, 956)),
        ] {
            let compatibility =
                RenderMetrics::from_drawable_size(logical.0, logical.1, drawable.0, drawable.1);
            let lifecycle = crate::videoio::RenderMetrics::from_sizes(
                (logical.0 as u32, logical.1 as u32),
                (drawable.0 as u32, drawable.1 as u32),
            );
            let display = crate::videoio_displays::RenderMetrics::new(
                logical.0, logical.1, drawable.0, drawable.1,
            );

            assert_eq!(
                (compatibility.logical_width, compatibility.logical_height),
                (
                    lifecycle.logical_width as i32,
                    lifecycle.logical_height as i32
                )
            );
            assert_eq!(
                (compatibility.drawable_width, compatibility.drawable_height),
                (
                    lifecycle.drawable_width as i32,
                    lifecycle.drawable_height as i32
                )
            );
            assert_eq!(compatibility.scale_x, lifecycle.scale_x);
            assert_eq!(compatibility.scale_y, lifecycle.scale_y);
            assert_eq!(
                (compatibility.logical_width, compatibility.logical_height),
                (display.logical_width, display.logical_height)
            );
            assert_eq!(
                (compatibility.drawable_width, compatibility.drawable_height),
                (display.drawable_width, display.drawable_height)
            );
            assert_eq!(compatibility.scale_x, display.scale_x);
            assert_eq!(compatibility.scale_y, display.scale_y);

            for value in [-1, 0, 1, 3, 10, 640] {
                assert_eq!(compatibility.scale_x(value), lifecycle.scale_x(value));
                assert_eq!(compatibility.scale_y(value), lifecycle.scale_y(value));
                assert_eq!(compatibility.scale_x(value), display.x(value));
                assert_eq!(compatibility.scale_y(value), display.y(value));
                assert_eq!(compatibility.scale_font(value), lifecycle.scale_font(value));
            }
        }
    }
}
