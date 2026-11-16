//! Radar reflectivity colours: the tables the tile services draw with, read back to
//! dBZ, and the classic NWS scale the app redraws them in.
//!
//! Both sources draw every pixel in one of their table's colours (checked against live
//! tiles), so a colour reads back exactly; anything else takes the nearest entry.

use std::collections::HashMap;
use std::sync::OnceLock;

/// RainViewer's Universal Blue scheme (colour 2, the only one its free API serves), as
/// RGBA, one entry per whole dBZ from -32 to 95: rain.
const RAINVIEWER_RAIN: [u32; 128] = [
    0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
    0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
    0x00000000, 0x00000000, 0x63615914, 0x66635a19, 0x69665c1e, 0x6c685d24, 0x6f6b5f29, 0x726e612e, 0x75706234, 0x78736439,
    0x7c75653e, 0x7f786744, 0x827b6949, 0x857d6a4e, 0x88806c54, 0x8b826d59, 0x8e856f5e, 0x92887164, 0x9e93756e, 0xaa9e7978,
    0xb6a97e82, 0xc2b4828c, 0xcec08796, 0xd2c48ba0, 0xd6c88faa, 0xdacc93b4, 0xded097be, 0x88ddeeff, 0x6cd1ebff, 0x51c5e8ff,
    0x36bae5ff, 0x1baee2ff, 0x00a3e0ff, 0x009ad5ff, 0x0091caff, 0x0088bfff, 0x007fb4ff, 0x0077aaff, 0x0070a3ff, 0x00699cff,
    0x006295ff, 0x005b8eff, 0x005588ff, 0x005180ff, 0x004e78ff, 0x004a70ff, 0x004768ff, 0xffee00ff, 0xffe000ff, 0xffd200ff,
    0xffc500ff, 0xffb700ff, 0xffaa00ff, 0xff9f00ff, 0xff9500ff, 0xff8b00ff, 0xff8100ff, 0xff4400ff, 0xf23600ff, 0xe62800ff,
    0xd91b00ff, 0xcd0d00ff, 0xc10000ff, 0xa80000ff, 0x8f0000ff, 0x760000ff, 0x5d0000ff, 0xffaaffff, 0xff9fffff, 0xff95ffff,
    0xff8bffff, 0xff81ffff, 0xff77ffff, 0xff6cffff, 0xff62ffff, 0xff58ffff, 0xff4effff, 0xffffffff, 0xffffffff, 0xffffffff,
    0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff,
    0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff,
    0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff, 0x00ff00ff,
];

/// The same scheme's snow colours (tiles requested with the snow option).
const RAINVIEWER_SNOW: [u32; 128] = [
    0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
    0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000,
    0x00000000, 0x00000000, 0xcfffff00, 0xceffff0c, 0xcdffff19, 0xccffff26, 0xcbffff33, 0xcbffff3f, 0xcaffff4c, 0xc9ffff59,
    0xc8ffff66, 0xc7ffff72, 0xc7ffff7f, 0xc6ffff8c, 0xc5ffff99, 0xc4ffffa5, 0xc3ffffb2, 0xc3ffffbf, 0xc2ffffcc, 0xc1ffffd8,
    0xc0ffffe5, 0xbffffff2, 0xbfffffff, 0xb8f8ffff, 0xb2f2ffff, 0xabebffff, 0xa5e5ffff, 0x9fdfffff, 0x98d8ffff, 0x92d2ffff,
    0x8bcbffff, 0x85c5ffff, 0x7fbfffff, 0x78b8ffff, 0x72b2ffff, 0x6babffff, 0x65a5ffff, 0x5f9fffff, 0x5b9bffff, 0x5898ffff,
    0x5595ffff, 0x5292ffff, 0x4f8fffff, 0x4b8bffff, 0x4888ffff, 0x4585ffff, 0x4282ffff, 0x3f7fffff, 0x3b7bffff, 0x3878ffff,
    0x3575ffff, 0x3272ffff, 0x2f6fffff, 0x2b6bffff, 0x2868ffff, 0x2565ffff, 0x2262ffff, 0x1f5fffff, 0x1b5bffff, 0x1858ffff,
    0x1555ffff, 0x1252ffff, 0x0f4fffff, 0x0c4bffff, 0x0948ffff, 0x0645ffff, 0x0242ffff, 0x003fffff, 0x003bffff, 0x0038ffff,
    0x0035ffff, 0x0032ffff, 0x002fffff, 0x002bffff, 0x0028ffff, 0x0025ffff, 0x0022ffff, 0x001fffff, 0x001bffff, 0x0018ffff,
    0x0015ffff, 0x0012ffff, 0x000fffff, 0x000cffff, 0x0009ffff, 0x0006ffff, 0x0002ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff,
    0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff,
    0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff, 0x0000ffff,
];

/// IEM's N0Q composite colours, as RGB: entry `i` is -32 + `i` / 2 dBZ (colour index
/// `i + 1`; index 0 is no data).
const IEM_N0Q: [u32; 255] = [
    0x85718f, 0x85728f, 0x86738d, 0x87758b, 0x87768b, 0x887789, 0x897987, 0x897a87, 0x8a7b85, 0x8b7d84, 0x8b7e84, 0x8c7f82,
    0x8d8180, 0x8d8280, 0x8e837e, 0x8f847c, 0x8f857c, 0x90877b, 0x918879, 0x918979, 0x928b77, 0x938d75, 0x969153, 0x989457,
    0x9b975b, 0x9d9a60, 0xa09d64, 0xa3a068, 0xa5a36d, 0xa8a671, 0xaaa976, 0xadac7a, 0xb0af7e, 0xb2b283, 0xb7b88c, 0xbabb90,
    0xbdbe94, 0xbfc199, 0xc2c49d, 0xc4c7a2, 0xc7caa6, 0xcacdaa, 0xccd0af, 0xd2d4b4, 0xcfd2b4, 0xc9ccb4, 0xc6c9b4, 0xc3c7b4,
    0xc0c4b4, 0xbdc1b4, 0xb9beb4, 0xb6bbb4, 0xb3b9b4, 0xb0b6b4, 0xadb3b4, 0xaab0b4, 0xa4abb4, 0xa0a8b4, 0x9da5b4, 0x9aa2b4,
    0x97a0b4, 0x949db4, 0x919ab4, 0x949bb5, 0x9098b4, 0x8c95b3, 0x8892b2, 0x808cb0, 0x7c89af, 0x7886ae, 0x7483ac, 0x7080ab,
    0x6c7daa, 0x6779a9, 0x6376a8, 0x5f73a7, 0x5b70a6, 0x576da4, 0x4f67a2, 0x4b64a1, 0x4761a0, 0x435e9f, 0x415b9e, 0x4361a2,
    0x4568a6, 0x486faa, 0x4a76ae, 0x4d7db2, 0x4f84b6, 0x518bbb, 0x5699c3, 0x599fc7, 0x5ba6cb, 0x5eadcf, 0x60b4d4, 0x62bbd8,
    0x65c2dc, 0x67c9e0, 0x6ad0e4, 0x6fd6e8, 0x68d6d7, 0x59d6b3, 0x52d6a2, 0x4bd690, 0x43d67e, 0x3cd66d, 0x35d65b, 0x11d518,
    0x11d117, 0x10cd17, 0x10c816, 0x10c416, 0x0fbc15, 0x0fb714, 0x0eb314, 0x0eaf13, 0x0eab13, 0x0da612, 0x0da212, 0x0d9e11,
    0x0c9911, 0x0c9510, 0x0c9110, 0x0b880f, 0x0b840e, 0x0a800e, 0x0a7c0d, 0x0a770d, 0x09730c, 0x096f0c, 0x096b0b, 0x08660b,
    0x08620a, 0x095e09, 0x327308, 0x467d08, 0x5b8807, 0x6f9207, 0x849d06, 0x98a806, 0xadb205, 0xc1bd05, 0xd6c704, 0xead204,
    0xffe200, 0xffd800, 0xffd300, 0xffce00, 0xffc900, 0xffc400, 0xffc000, 0xffbb00, 0xffb600, 0xffb100, 0xffac00, 0xffa700,
    0xffa200, 0xff9900, 0xff9400, 0xff8f00, 0xff8a00, 0xff8500, 0xff8000, 0xff0000, 0xf80000, 0xf10000, 0xea0000, 0xe30000,
    0xd50000, 0xcd0000, 0xc60000, 0xbf0000, 0xb80000, 0xb10000, 0xaa0000, 0xa30000, 0x9b0000, 0x940000, 0x8d0000, 0x7f0000,
    0x780000, 0x710000, 0xffffff, 0xfff5ff, 0xffeaff, 0xffdfff, 0xffd4ff, 0xffc9ff, 0xffbeff, 0xffb3ff, 0xff9dff, 0xff92ff,
    0xff75ff, 0xfc6bfd, 0xf960fa, 0xf656f7, 0xf34bf4, 0xf040f1, 0xed36ef, 0xea2bec, 0xe720e9, 0xe10be3, 0xb200ff, 0xac00fc,
    0xa400f7, 0x9b00f4, 0x9300ef, 0x8800ea, 0x8300e8, 0x7900e2, 0x7200dd, 0x6900db, 0x05ecf0, 0x05ebf0, 0x05eaf0, 0x05dde0,
    0x05dce0, 0x05dbe0, 0x05cdd0, 0x05ccd0, 0x04bdc0, 0x04bcc0, 0x04bbc0, 0x04aeb0, 0x04adb0, 0x049ea0, 0x049da0, 0x049ca0,
    0x038e90, 0x038d90, 0x038c90, 0x037e80, 0x037d80, 0x036f70, 0x036e70, 0x036d70, 0x025f60, 0x025e60, 0x024f50, 0x024e50,
    0x024d50, 0x023f40, 0x023e40, 0x023d40, 0x013030, 0x012f30, 0x012020, 0x011f20, 0x011e20, 0x3a67b5, 0x3a66b5, 0x3a65b5,
    0x3a64b5, 0x3a63b5, 0x3a62b5,
];

/// Which service drew a radar tile, so its colours can be read back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RadarPalette {
    RainViewer,
    Iem,
}

/// One radar reading: reflectivity in dBZ, and whether the service marked it as snow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Echo {
    pub dbz: f32,
    pub snow: bool,
}

/// The classic NWS reflectivity scale (dBZ, RGB, opacity), blended between stops. Under
/// 15 dBZ it fades out, so drizzle and ground clutter don't haze the map.
const RAIN_STOPS: [(f32, u32, f32); 16] = [
    (4.0, 0x04e9e7, 0.0),
    (5.0, 0x04e9e7, 0.14),
    (10.0, 0x019ff4, 0.26),
    (15.0, 0x0300f4, 0.42),
    (20.0, 0x02fd02, 0.92),
    (25.0, 0x01c501, 1.0),
    (30.0, 0x008e00, 1.0),
    (35.0, 0xfdf802, 1.0),
    (40.0, 0xe5bc00, 1.0),
    (45.0, 0xfd9500, 1.0),
    (50.0, 0xfd0000, 1.0),
    (55.0, 0xd40000, 1.0),
    (60.0, 0xbc0000, 1.0),
    (65.0, 0xf800fd, 1.0),
    (70.0, 0x9854c6, 1.0),
    (75.0, 0xfdfdfd, 1.0),
];

/// Snow keeps a cool ramp: pale blue to deep blue to violet.
const SNOW_STOPS: [(f32, u32, f32); 8] = [
    (4.0, 0xcfe9ff, 0.0),
    (5.0, 0xcfe9ff, 0.2),
    (10.0, 0xa8d4ff, 0.35),
    (15.0, 0x7fb8ff, 0.6),
    (20.0, 0x5a9bff, 0.92),
    (30.0, 0x2f6be8, 1.0),
    (45.0, 0x1b3fc0, 1.0),
    (60.0, 0x8a2be2, 1.0),
];

fn rgb(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

fn ramp(stops: &[(f32, u32, f32)], dbz: f32) -> [u8; 4] {
    let (first, last) = (stops[0], stops[stops.len() - 1]);
    if dbz < first.0 {
        return [0; 4];
    }
    let (lo, hi, t) = match stops.windows(2).find(|w| dbz < w[1].0) {
        Some(w) => (w[0], w[1], (dbz - w[0].0) / (w[1].0 - w[0].0)),
        None => (last, last, 0.0),
    };
    let (a, b) = (rgb(lo.1), rgb(hi.1));
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    let alpha = lo.2 + (hi.2 - lo.2) * t;
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]), (alpha * 255.0).round() as u8]
}

/// The colour (straight RGBA) to draw `echo` in.
pub fn nws_color(echo: Echo) -> [u8; 4] {
    if echo.snow { ramp(&SNOW_STOPS, echo.dbz) } else { ramp(&RAIN_STOPS, echo.dbz) }
}

/// Every colour `palette` draws with, and the reading it stands for. RainViewer's keys are
/// RGBA (its faint colours differ only in opacity); IEM's are RGB. Rain wins where a rain
/// and a snow colour are the same.
fn entries(palette: RadarPalette) -> Vec<(u32, Echo)> {
    match palette {
        RadarPalette::RainViewer => {
            let mut out = Vec::new();
            for (snow, table) in [(false, &RAINVIEWER_RAIN), (true, &RAINVIEWER_SNOW)] {
                for (i, &c) in table.iter().enumerate() {
                    if c & 0xff != 0 {
                        out.push((c, Echo { dbz: i as f32 - 32.0, snow }));
                    }
                }
            }
            out
        }
        RadarPalette::Iem => {
            IEM_N0Q.iter().enumerate().map(|(i, &c)| (c, Echo { dbz: i as f32 / 2.0 - 32.0, snow: false })).collect()
        }
    }
}

/// The key a pixel is looked up by in `palette`'s table.
fn key(palette: RadarPalette, px: [u8; 4]) -> u32 {
    let rgb = (px[0] as u32) << 16 | (px[1] as u32) << 8 | px[2] as u32;
    match palette {
        RadarPalette::RainViewer => rgb << 8 | px[3] as u32,
        RadarPalette::Iem => rgb,
    }
}

struct Table {
    exact: HashMap<u32, Echo>,
    entries: Vec<(u32, Echo)>,
}

fn table(palette: RadarPalette) -> &'static Table {
    static RAINVIEWER: OnceLock<Table> = OnceLock::new();
    static IEM: OnceLock<Table> = OnceLock::new();
    let cell = match palette {
        RadarPalette::RainViewer => &RAINVIEWER,
        RadarPalette::Iem => &IEM,
    };
    cell.get_or_init(|| {
        let entries = entries(palette);
        let mut exact = HashMap::new();
        for &(k, e) in &entries {
            exact.entry(k).or_insert(e);
        }
        Table { exact, entries }
    })
}

/// What the straight-RGBA pixel `px` of a `palette` tile reads as; `None` where there's
/// no echo.
pub fn decode(palette: RadarPalette, px: [u8; 4]) -> Option<Echo> {
    if px[3] == 0 {
        return None;
    }
    let t = table(palette);
    let k = key(palette, px);
    if let Some(e) = t.exact.get(&k) {
        return Some(*e);
    }
    let channels = |c: u32| -> [i32; 4] {
        match palette {
            RadarPalette::RainViewer => [(c >> 24) as i32, (c >> 16 & 0xff) as i32, (c >> 8 & 0xff) as i32, (c & 0xff) as i32],
            RadarPalette::Iem => [(c >> 16) as i32, (c >> 8 & 0xff) as i32, (c & 0xff) as i32, 255],
        }
    };
    let want = channels(k);
    t.entries
        .iter()
        .min_by_key(|(c, _)| {
            let have = channels(*c);
            (0..4).map(|i| (have[i] - want[i]).pow(2)).sum::<i32>()
        })
        .map(|(_, e)| *e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(c: u32, palette: RadarPalette) -> [u8; 4] {
        match palette {
            RadarPalette::RainViewer => [(c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8, c as u8],
            RadarPalette::Iem => [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255],
        }
    }

    #[test]
    fn table_colours_read_back_as_their_dbz() {
        for (i, &c) in RAINVIEWER_RAIN.iter().enumerate().filter(|(i, _)| (22..=96).contains(i)) {
            let e = decode(RadarPalette::RainViewer, px(c, RadarPalette::RainViewer)).unwrap();
            // Neighbouring dBZ share a colour in places (65 and 70 are both white).
            assert!((e.dbz - (i as f32 - 32.0)).abs() <= 5.0 && !e.snow, "rain {i}: {e:?}");
        }
        for (i, &c) in RAINVIEWER_SNOW.iter().enumerate().filter(|(i, c)| (32..=100).contains(i) && **c & 0xff != 0) {
            let e = decode(RadarPalette::RainViewer, px(c, RadarPalette::RainViewer)).unwrap();
            assert!((e.dbz - (i as f32 - 32.0)).abs() <= 5.0, "snow {i}: {e:?}");
        }
        for (i, &c) in IEM_N0Q.iter().enumerate() {
            let e = decode(RadarPalette::Iem, px(c, RadarPalette::Iem)).unwrap();
            assert!((e.dbz - (i as f32 / 2.0 - 32.0)).abs() <= 1.0, "iem {i}: {e:?}");
        }
    }

    #[test]
    fn colours_off_the_table_take_the_nearest() {
        let c = RAINVIEWER_RAIN[(40 + 32) as usize];
        let mut p = px(c, RadarPalette::RainViewer);
        p[0] = p[0].saturating_sub(3);
        let e = decode(RadarPalette::RainViewer, p).unwrap();
        assert!((e.dbz - 40.0).abs() <= 2.0);
        assert_eq!(decode(RadarPalette::Iem, [0, 0, 0, 0]), None);
    }

    #[test]
    fn the_scale_fades_in_then_holds() {
        let at = |d: f32| nws_color(Echo { dbz: d, snow: false });
        assert_eq!(at(0.0)[3], 0);
        let alphas: Vec<u8> = (0..=75).map(|d| at(d as f32)[3]).collect();
        assert!(alphas.windows(2).all(|w| w[1] >= w[0]), "opacity never drops as dBZ rises");
        assert_eq!(at(30.0), [0x00, 0x8e, 0x00, 255]);
        assert_eq!(at(50.0), [0xfd, 0x00, 0x00, 255]);
        assert!(nws_color(Echo { dbz: 30.0, snow: true })[2] > 200, "snow stays blue");
    }
}
