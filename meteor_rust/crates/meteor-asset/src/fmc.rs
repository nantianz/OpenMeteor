use meteor_core::{Quat, Vec3};

#[derive(Debug, Clone, PartialEq)]
pub struct FmcPart {
    pub t: Vec3,

    pub q: Quat,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FmcFrame {
    pub index: u32,

    pub parts: Vec<FmcPart>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FmcClip {
    pub fps: f32,

    pub frame_count: u32,

    pub mode: String,
    pub scene_object_count: u32,
    pub dummy_object_count: u32,

    pub frames: Vec<FmcFrame>,
}

impl FmcClip {
    pub fn sample_part(&self, frame: f32, part_index: usize) -> Option<FmcPart> {
        let index = (frame.max(0.0).floor() as usize).min(self.frames.len().checked_sub(1)?);
        let current = self.frames[index].parts.get(part_index)?;
        let next = self
            .frames
            .get(index + 1)
            .and_then(|next| next.parts.get(part_index))
            .unwrap_or(current);
        let fraction = (frame - index as f32).clamp(0.0, 1.0);
        Some(FmcPart {
            t: current.t + (next.t - current.t) * fraction,
            q: current.q.slerp(&next.q, fraction),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropPose {
    pub start: u32,
    pub end: u32,
}

pub fn parse_fmc(text: &str) -> FmcClip {
    let header = find_number("SceneObjects", text).unwrap_or(0.0) as u32;
    let dummy = find_number("DummeyObjects", text).unwrap_or(0.0) as u32;
    let fps = find_number("FPS", text).unwrap_or(30.0);
    let frame_count = find_number("Frames", text).unwrap_or(0.0) as u32;
    let mode = find_mode(text);

    let mut frames = Vec::new();

    let mut rest = text;
    while let Some(start) = rest.find("frame ") {
        rest = &rest[start + 6..];
        let Some(num_end) = rest.find(|c: char| !c.is_ascii_digit()) else {
            break;
        };
        let index: u32 = rest[..num_end].parse().unwrap_or(0);
        rest = &rest[num_end..];
        let Some(open) = rest.find('{') else {
            break;
        };
        let Some(close) = rest[open + 1..].find('}') else {
            break;
        };
        let body = &rest[open + 1..open + 1 + close];
        rest = &rest[open + 1 + close..];
        frames.push(FmcFrame {
            index,
            parts: parse_parts(body),
        });
    }
    frames.sort_by_key(|f| f.index);
    FmcClip {
        fps,
        frame_count: if frame_count > 0 {
            frame_count
        } else {
            frames.len() as u32
        },
        mode,
        scene_object_count: header,
        dummy_object_count: dummy,
        frames,
    }
}

fn parse_parts(body: &str) -> Vec<FmcPart> {
    let mut parts = Vec::new();
    let mut rest = body;
    while let Some(t_pos) = rest.find(['t', 'T']) {
        rest = &rest[t_pos + 1..];
        let after = rest.trim_start();

        let tokens: Vec<&str> = after.split_whitespace().take(9).collect();
        if tokens.len() < 8 {
            break;
        }
        let nums: Vec<f32> = tokens[0..3]
            .iter()
            .chain(&tokens[4..8])
            .filter_map(|s| s.parse().ok())
            .collect();
        if nums.len() < 7 {
            break;
        }
        let (tx, ty, tz) = (nums[0], nums[1], nums[2]);

        let (qw, qx, qy, qz) = (nums[3], nums[4], nums[5], nums[6]);
        parts.push(FmcPart {
            t: Vec3::new(tx, ty, tz),
            q: Quat {
                x: qx,
                y: qy,
                z: qz,
                w: qw,
            },
        });
        rest = after;
    }
    parts
}

pub fn parse_prop_poses(text: &str) -> Vec<PropPose> {
    let mut poses = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("pose") {
        rest = &rest[start + 4..];
        let Some(open) = rest.find('{') else {
            break;
        };
        let Some(close) = rest[open + 1..].find('}') else {
            break;
        };
        let body = &rest[open + 1..open + 1 + close];
        rest = &rest[open + 1 + close..];
        let start_frame = find_number("start", body).unwrap_or(0.0) as u32;
        let end_frame = find_number("end", body)
            .map(|v| v as u32)
            .unwrap_or(start_frame);
        poses.push(PropPose {
            start: start_frame,
            end: end_frame,
        });
    }
    poses
}

fn find_number(keyword: &str, text: &str) -> Option<f32> {
    let lower = text.to_ascii_lowercase();
    let key = keyword.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(pos) = rest.find(key.as_str()) {
        rest = &rest[pos + key.len()..];
        let after = rest.trim_start();
        let num_end = after
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
            .unwrap_or(after.len());
        if num_end > 0 {
            if let Ok(v) = after[..num_end].trim().parse() {
                return Some(v);
            }
        }
    }
    None
}

fn find_mode(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if let Some(pos) = lower.find("mode:") {
        let after = lower[pos + 5..].trim_start();
        let end = after
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(after.len());
        if end > 0 {
            return after[..end].to_uppercase();
        }
    }
    "RIGID".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# GModel Animation File V1.0
SceneObjects 2 DummeyObjects 0
FPS 30
Frames 2
Mode: RIGID

frame 0 {
t 10 -5 3 q -1 0 0 0
t 0 0 0 q 1 0 0 0
}
frame 1 {
t 11 -4 3.5 q -1 0 0 0
t 0 0 0 q 1 0 0 0
}
"#;

    #[test]
    fn parses_header_and_frames() {
        let clip = parse_fmc(SAMPLE);
        assert_eq!(clip.scene_object_count, 2);
        assert_eq!(clip.fps, 30.0);
        assert_eq!(clip.frame_count, 2);
        assert_eq!(clip.mode, "RIGID");
        assert_eq!(clip.frames.len(), 2);
        assert_eq!(clip.frames[0].index, 0);
        assert_eq!(clip.frames[1].index, 1);
    }

    #[test]
    fn fractional_rigid_sample_interpolates_and_holds_last_frame() {
        let clip = parse_fmc("SceneObjects 1 FPS 30 Frames 2\nframe 0 { t 0 0 0 q 1 0 0 0 }\nframe 1 { t 10 20 30 q 0 0 0 -1 }");
        let half = clip.sample_part(0.5, 0).unwrap();
        assert_eq!(half.t, Vec3::new(5.0, 10.0, 15.0));
        assert!((half.q.w.abs() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        assert!((half.q.z.abs() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        assert_eq!(clip.sample_part(1.5, 0).unwrap(), clip.frames[1].parts[0]);
        assert!(clip.sample_part(0.5, 1).is_none());
    }

    #[test]
    fn parses_parts_in_des_order() {
        let clip = parse_fmc(SAMPLE);
        let frame = &clip.frames[0];
        assert_eq!(frame.parts.len(), 2);

        let p0 = &frame.parts[0];
        assert!((p0.t.x - 10.0).abs() < 1e-5);
        assert!((p0.t.y + 5.0).abs() < 1e-5);
        assert!((p0.t.z - 3.0).abs() < 1e-5);
        assert!((p0.q.x - 0.0).abs() < 1e-5);
        assert!((p0.q.w + 1.0).abs() < 1e-5);

        let p1 = &frame.parts[1];
        assert!((p1.q.w - 1.0).abs() < 1e-5);
    }

    #[test]
    fn frame_delta_between_consecutive_frames() {
        let clip = parse_fmc(SAMPLE);
        let f0 = &clip.frames[0].parts[0];
        let f1 = &clip.frames[1].parts[0];
        assert!((f1.t.x - f0.t.x - 1.0).abs() < 1e-5);
        assert!((f1.t.z - f0.t.z - 0.5).abs() < 1e-5);
    }

    #[test]
    fn missing_frames_header_falls_back_to_length() {
        let text = "# GModel Animation File V1.0\nSceneObjects 1 DummeyObjects 0\nframe 0 { t 0 0 0 q 1 0 0 0 }\n";
        let clip = parse_fmc(text);
        assert_eq!(clip.frame_count, 1);
        assert_eq!(clip.frames.len(), 1);
    }

    #[test]
    fn parses_prop_poses() {
        let text = "pose { start 0 end 5 }\npose { start 6 end 12 }\n";
        let poses = parse_prop_poses(text);
        assert_eq!(poses.len(), 2);
        assert_eq!(poses[0], PropPose { start: 0, end: 5 });
        assert_eq!(poses[1], PropPose { start: 6, end: 12 });
    }
}
