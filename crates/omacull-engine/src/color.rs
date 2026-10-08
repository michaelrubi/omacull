//! Showing a frame in the monitor's colours.
//!
//! The camera's JPEGs are sRGB or Adobe RGB, as the camera was set; a full
//! decode of the raw is developed to sRGB. Hyprland takes what Omacull
//! draws for sRGB, so on a wide-gamut monitor that isn't sRGB every colour
//! comes out too saturated unless it's converted to the monitor's profile
//! first. Profiles and EDID reading as in Omapix.
//!
//! A JPEG on its own may carry a profile of its own, and be in anything:
//! darktable exports in whatever it's told to. [`tagged`] brings those to
//! sRGB as they're decoded, so the rest of Omacull only knows two spaces.

use std::io::{self, ErrorKind};

use lcms2::{
    CIExyY, CIExyYTRIPLE, DisallowCache, Flags, GlobalContext, InfoType, Intent, Locale, MLU, PixelFormat, Profile,
    Tag, TagSignature, ToneCurve, Transform,
};

use rayon::prelude::*;

use crate::image::Image;

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, e.to_string())
}

/// The colours a camera JPEG or a developed raw is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Srgb,
    AdobeRgb,
}

/// A monitor's colours, as an ICC profile.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorProfile {
    icc: Vec<u8>,
    description: String,
}

fn described(mut profile: Profile, description: String) -> io::Result<ColorProfile> {
    let mut mlu = MLU::new(1);
    mlu.set_text(&description, Locale::none());
    profile.write_tag(TagSignature::ProfileDescriptionTag, Tag::MLU(&mlu));
    Ok(ColorProfile { icc: profile.icc().map_err(invalid)?, description })
}

fn point((x, y): (f64, f64)) -> CIExyY {
    CIExyY { x, y, Y: 1.0 }
}

impl ColorProfile {
    pub fn from_icc(icc: Vec<u8>) -> io::Result<Self> {
        let profile = Profile::new_icc(&icc).map_err(invalid)?;
        let description = profile.info(InfoType::Description, Locale::none()).unwrap_or_else(|| "ICC profile".into());
        Ok(Self { icc, description })
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn icc(&self) -> &[u8] {
        &self.icc
    }

    /// Adobe RGB (1998): its primaries, D65 white and a 2.2 gamma.
    pub fn adobe_rgb() -> Self {
        let (red, green, blue) = (point((0.64, 0.33)), point((0.21, 0.71)), point((0.15, 0.06)));
        let primaries = CIExyYTRIPLE { Red: red, Green: green, Blue: blue };
        let curve = ToneCurve::new(563.0 / 256.0);
        let profile = Profile::new_rgb(&point((0.3127, 0.3290)), &primaries, &[&curve, &curve, &curve])
            .expect("Adobe RGB is a valid profile");
        described(profile, "Adobe RGB (1998)".into()).expect("Adobe RGB serialises")
    }

    /// A monitor's colours as its EDID gives them (`name` is the monitor's,
    /// for the description): the red, green, blue and white it shows, as a
    /// matrix profile. `None` if the EDID has none, or they aren't
    /// believable as a monitor's (many report zeros or leftovers).
    ///
    /// Its tone curve is sRGB's when the EDID says gamma 2.2 (or nothing),
    /// which is a nominal figure: greys then come out as they would with no
    /// profile, and only colours change.
    pub fn from_edid(edid: &[u8], name: &str) -> Option<Self> {
        const HEADER: [u8; 8] = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];
        if edid.len() < 128 || edid[..8] != HEADER {
            return None;
        }
        // Ten bits each: the low two packed into bytes 25 and 26.
        let xy = |i: usize| {
            let low = u16::from(edid[25 + i / 4] >> (6 - 2 * (i % 4))) & 3;
            f64::from(u16::from(edid[27 + i]) << 2 | low) / 1024.0
        };
        let [red, green, blue, white] = [0, 2, 4, 6].map(|i| (xy(i), xy(i + 1)));
        let area = ((green.0 - red.0) * (blue.1 - red.1) - (blue.0 - red.0) * (green.1 - red.1)).abs() / 2.0;
        let believable = red.0 > 0.55
            && red.1 < 0.4
            && green.1 > 0.5
            && blue.0 < 0.25
            && blue.1 < 0.2
            && (0.25..0.4).contains(&white.0)
            && (0.25..0.42).contains(&white.1)
            && [red, green, blue].iter().all(|c| c.1 > 0.0 && c.0 + c.1 <= 1.0)
            // sRGB's triangle is 0.112; Rec. 2020's 0.212.
            && (0.07..0.25).contains(&area);
        if !believable {
            return None;
        }
        let gamma = match edid[23] {
            0xff => 2.2,
            g => (f64::from(g) + 100.0) / 100.0,
        };
        let curve = if (gamma - 2.2).abs() < 0.05 {
            ToneCurve::new_parametric(4, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]).ok()?
        } else {
            ToneCurve::new(gamma)
        };
        let primaries = CIExyYTRIPLE { Red: point(red), Green: point(green), Blue: point(blue) };
        let profile = Profile::new_rgb(&point(white), &primaries, &[&curve, &curve, &curve]).ok()?;
        described(profile, format!("{name} (EDID)")).ok()
    }

    fn lcms(&self) -> io::Result<Profile> {
        Profile::new_icc(&self.icc).map_err(invalid)
    }
}

type Rgba = Transform<[u8; 4], [u8; 4], GlobalContext, DisallowCache>;

/// Converts frames to the monitor's colours.
pub struct Display {
    /// What the monitor is; None for sRGB.
    monitor: Option<ColorProfile>,
    /// From sRGB, unless the monitor is sRGB.
    from_srgb: Option<Rgba>,
    from_adobe: Rgba,
}

fn transform(from: &Profile, to: &Profile) -> io::Result<Rgba> {
    Transform::new_flags_context(
        GlobalContext::new(),
        from,
        PixelFormat::RGBA_8,
        to,
        PixelFormat::RGBA_8,
        Intent::Perceptual,
        Flags::NO_CACHE,
    )
    .map_err(invalid)
}

/// What a picture that carries its own profile is in. If that's sRGB or
/// Adobe RGB it's left as it is; from anything else it's converted to
/// sRGB, in place, so it's measured and shown like any other. None if the
/// profile can't be made sense of.
pub fn tagged(image: &mut Image, icc: &[u8]) -> Option<Space> {
    // Colours that come through unchanged only if the two are the same.
    const PROBES: [[u8; 4]; 8] = [
        [0, 0, 0, 255],
        [255, 255, 255, 255],
        [128, 128, 128, 255],
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [64, 128, 192, 255],
        [200, 100, 50, 255],
    ];
    let profile = Profile::new_icc(icc).ok()?;
    let same = |to: &Rgba| {
        let mut probes = PROBES;
        to.transform_in_place(&mut probes);
        probes.iter().zip(&PROBES).all(|(got, sent)| got.iter().zip(sent).all(|(a, b)| a.abs_diff(*b) <= 2))
    };
    let to_srgb = transform(&profile, &Profile::new_srgb()).ok()?;
    if same(&to_srgb) {
        return Some(Space::Srgb);
    }
    if same(&transform(&profile, &ColorProfile::adobe_rgb().lcms().ok()?).ok()?) {
        return Some(Space::AdobeRgb);
    }
    // A few rows at a time, on every core: a full-size frame is 24 MP.
    image.rgba.par_chunks_mut(1 << 16).for_each(|part| to_srgb.transform_in_place(part.as_chunks_mut::<4>().0));
    Some(Space::Srgb)
}

impl Display {
    /// For a monitor with this profile; None for sRGB.
    pub fn new(monitor: Option<ColorProfile>) -> io::Result<Self> {
        let to = match &monitor {
            Some(profile) => profile.lcms()?,
            None => Profile::new_srgb(),
        };
        let from_srgb = monitor.as_ref().map(|_| transform(&Profile::new_srgb(), &to)).transpose()?;
        let from_adobe = transform(&ColorProfile::adobe_rgb().lcms()?, &to)?;
        Ok(Self { monitor, from_srgb, from_adobe })
    }

    /// For an sRGB monitor: sRGB is shown as it is.
    pub fn srgb() -> Self {
        Self::new(None).expect("sRGB and Adobe RGB are valid profiles")
    }

    pub fn monitor(&self) -> Option<&ColorProfile> {
        self.monitor.as_ref()
    }

    /// Convert pixels in `space` to the monitor's colours, in place.
    pub fn convert(&self, image: &mut Image, space: Space) {
        let transform = match space {
            Space::Srgb => match &self.from_srgb {
                Some(t) => t,
                None => return,
            },
            Space::AdobeRgb => &self.from_adobe,
        };
        let (pixels, _) = image.rgba.as_chunks_mut::<4>();
        transform.transform_in_place(pixels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(rgb: [u8; 3]) -> Image {
        Image { width: 1, height: 1, rgba: vec![rgb[0], rgb[1], rgb[2], 255] }
    }

    #[test]
    fn srgb_on_an_srgb_monitor_is_left_alone_and_adobe_rgb_is_converted() {
        let display = Display::srgb();
        let mut red = one([200, 30, 30]);
        display.convert(&mut red, Space::Srgb);
        assert_eq!(red.rgba, [200, 30, 30, 255]);
        // Adobe RGB's red is more saturated than sRGB can show.
        let mut adobe = one([200, 30, 30]);
        display.convert(&mut adobe, Space::AdobeRgb);
        assert!(adobe.rgba[0] > 200 && adobe.rgba[1] < 30, "{:?}", adobe.rgba);
        assert_eq!(adobe.rgba[3], 255, "alpha is kept");
        // Greys are greys in both.
        let mut grey = one([128, 128, 128]);
        display.convert(&mut grey, Space::AdobeRgb);
        let [r, g, b, _] = grey.rgba[..] else { unreachable!() };
        assert!(r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1, "{:?}", grey.rgba);
    }

    #[test]
    fn on_a_wide_gamut_monitor_srgb_is_less_saturated() {
        // A wide-gamut monitor shows its pure green more saturated than
        // sRGB's, so sRGB's green needs some red and blue in it.
        let display = Display::new(Some(ColorProfile::adobe_rgb())).unwrap();
        let mut green = one([0, 255, 0]);
        display.convert(&mut green, Space::Srgb);
        let [r, g, b, _] = green.rgba[..] else { unreachable!() };
        assert!(r > 100 && g == 255 && b > 30, "{:?}", green.rgba);
        let mut adobe = one([255, 0, 0]);
        display.convert(&mut adobe, Space::AdobeRgb);
        assert_eq!(adobe.rgba[..3], [255, 0, 0], "Adobe RGB on an Adobe RGB monitor is unchanged");
        assert_eq!(display.monitor().unwrap().description(), "Adobe RGB (1998)");
    }

    #[test]
    fn a_picture_with_its_own_profile_is_brought_to_srgb() {
        let pixel = [128, 128, 128];
        let mut image = one(pixel);
        let srgb = Profile::new_srgb().icc().unwrap();
        assert_eq!(tagged(&mut image, &srgb), Some(Space::Srgb));
        assert_eq!(tagged(&mut image, ColorProfile::adobe_rgb().icc()), Some(Space::AdobeRgb));
        assert_eq!(image, one(pixel), "sRGB and Adobe RGB are left as they are");

        // Linear, as darktable exports when it's told to: mid-grey is far
        // brighter once it's sRGB.
        let (red, green, blue) = (point((0.64, 0.33)), point((0.30, 0.60)), point((0.15, 0.06)));
        let primaries = CIExyYTRIPLE { Red: red, Green: green, Blue: blue };
        let curve = ToneCurve::new(1.0);
        let linear = Profile::new_rgb(&point((0.3127, 0.3290)), &primaries, &[&curve, &curve, &curve]).unwrap();
        assert_eq!(tagged(&mut image, &linear.icc().unwrap()), Some(Space::Srgb));
        let [r, g, b, a] = image.rgba[..] else { unreachable!() };
        assert!((186..=190).contains(&g) && r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1 && a == 255, "{:?}", image.rgba);

        assert_eq!(tagged(&mut image, b"not a profile"), None);
    }

    /// An EDID header and chromaticities, as a monitor reports them.
    fn edid(colours: [(f64, f64); 4], gamma: u8) -> Vec<u8> {
        let mut edid = vec![0u8; 128];
        edid[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
        edid[23] = gamma;
        for (i, value) in colours.iter().flat_map(|&(x, y)| [x, y]).enumerate() {
            let bits = (value * 1024.0).round() as u16;
            edid[27 + i] = (bits >> 2) as u8;
            edid[25 + i / 4] |= ((bits & 3) as u8) << (6 - 2 * (i % 4));
        }
        edid
    }

    #[test]
    fn edid_colours_make_a_profile_when_theyre_believable() {
        let p3 = [(0.68, 0.32), (0.265, 0.69), (0.15, 0.06), (0.3127, 0.329)];
        let profile = ColorProfile::from_edid(&edid(p3, 120), "eDP-1").unwrap();
        assert_eq!(profile.description(), "eDP-1 (EDID)");
        assert!(ColorProfile::from_edid(&edid([(0.0, 0.0); 4], 120), "m").is_none());
        assert!(ColorProfile::from_edid(&edid(p3, 120)[..100], "m").is_none());
        assert!(ColorProfile::from_icc(profile.icc.clone()).is_ok());
        assert!(ColorProfile::from_icc(b"not a profile".to_vec()).is_err());
    }
}
