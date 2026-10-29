use bevy_math::{Affine3A, Mat4, Vec3, Vec4};

pub struct ViewRangefinder3d {
    view_from_world_row_2: Vec4,
}

impl ViewRangefinder3d {

    pub fn from_world_from_view(world_from_view: &Affine3A) -> ViewRangefinder3d {
        let view_from_world = world_from_view.inverse();

        ViewRangefinder3d {
            view_from_world_row_2: Mat4::from(view_from_world).row(2),
        }
    }

    #[inline]
    pub fn distance(&self, position: &Vec3) -> f32 {

        self.view_from_world_row_2.dot(position.extend(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::ViewRangefinder3d;
    use bevy_math::{Affine3A, Vec3};

    #[test]
    fn distance() {
        let view_matrix = Affine3A::from_translation(Vec3::new(0.0, 0.0, -1.0));
        let rangefinder = ViewRangefinder3d::from_world_from_view(&view_matrix);
        assert_eq!(rangefinder.distance(&Vec3::new(0., 0., 0.)), 1.0);
        assert_eq!(rangefinder.distance(&Vec3::new(0., 0., 1.)), 2.0);
    }
}
