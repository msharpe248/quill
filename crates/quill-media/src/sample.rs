//! A picture made here, for proving that the terminal draws images before
//! messages have any (M4).

use crate::Image;

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 160;

/// A sunset over hills: a sky that fades from deep blue to orange, a sun, and
/// two ridges. Enough shape and colour to see at once whether it arrived
/// whole, the right way up and unstretched.
pub fn image() -> Image {
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    let (sun_x, sun_y, sun_radius) = (230.0_f32, 92.0_f32, 26.0_f32);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (fx, fy) = (x as f32, y as f32);
            let t = fy / HEIGHT as f32;
            // Sky: deep blue at the top, through violet, to orange.
            let mut pixel = [
                lerp(26.0, 247.0, t.powf(1.6)),
                lerp(27.0, 140.0, t.powf(2.2)),
                lerp(60.0, 80.0, t),
            ];
            let distance = ((fx - sun_x).powi(2) + (fy - sun_y).powi(2)).sqrt();
            if distance < sun_radius {
                pixel = [255.0, 226.0, 150.0];
            } else if distance < sun_radius * 2.2 {
                // A glow that fades with distance.
                let glow = 1.0 - (distance - sun_radius) / (sun_radius * 1.2);
                pixel[0] = lerp(pixel[0], 255.0, glow * 0.45);
                pixel[1] = lerp(pixel[1], 200.0, glow * 0.35);
            }
            let far_ridge = 104.0 + 14.0 * (fx / 46.0).sin() + 6.0 * (fx / 17.0).sin();
            let near_ridge = 128.0 + 18.0 * (fx / 71.0 + 1.3).sin() + 5.0 * (fx / 23.0).cos();
            if fy > near_ridge {
                pixel = [24.0, 26.0, 44.0];
            } else if fy > far_ridge {
                pixel = [70.0, 58.0, 96.0];
            }
            rgba.extend(pixel.map(|channel| channel.clamp(0.0, 255.0) as u8));
            rgba.push(255);
        }
    }
    Image {
        width: WIDTH,
        height: HEIGHT,
        rgba,
    }
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.width + x) * 4) as usize;
        [
            image.rgba[at],
            image.rgba[at + 1],
            image.rgba[at + 2],
            image.rgba[at + 3],
        ]
    }

    #[test]
    fn the_sample_is_whole_and_opaque() {
        let image = image();
        assert!(Image::new(image.width, image.height, image.rgba.clone()).is_some());
        assert!(image.rgba.chunks(4).all(|pixel| pixel[3] == 255));
    }

    #[test]
    fn the_sample_has_a_top_a_bottom_and_a_sun() {
        let image = image();
        // Blue sky at the top, dark hills at the bottom, a pale sun between.
        let sky = pixel(&image, 10, 2);
        assert!(sky[2] > sky[0], "{sky:?}");
        assert_eq!(pixel(&image, 10, HEIGHT - 1), [24, 26, 44, 255]);
        assert_eq!(pixel(&image, 230, 80), [255, 226, 150, 255]);
        // Not symmetrical left to right, so a mirrored image shows.
        assert_ne!(pixel(&image, 90, 80), pixel(&image, 230, 80));
    }
}
