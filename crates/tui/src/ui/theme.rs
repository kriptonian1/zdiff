//! The built-in color themes.

use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use zdiff_core::Status;
use zdiff_highlight::Class;

/// A built-in color theme; the settings file stores it by name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    GithubDark,
    GithubLight,
    GithubDarkDimmed,
    GithubHighContrastDark,
    GithubHighContrastLight,
    OneDark,
    OneLight,
    Dracula,
    Monokai,
    MonokaiPro,
    TokyoNight,
    TokyoNightStorm,
    TokyoNightMoon,
    TokyoNightDay,
    CatppuccinMocha,
    CatppuccinMacchiato,
    CatppuccinFrappe,
    CatppuccinLatte,
    Nord,
    GruvboxDark,
    GruvboxLight,
    SolarizedDark,
    SolarizedLight,
    RosePine,
    RosePineMoon,
    RosePineDawn,
    Kanagawa,
    EverforestDark,
    EverforestLight,
    AyuDark,
    AyuMirage,
    AyuLight,
    NightOwl,
    NightOwlLight,
    MaterialOcean,
    Palenight,
    VsCodeDark,
    VsCodeLight,
    Nightfox,
    OceanicNext,
    Cobalt2,
    Synthwave84,
    ShadesOfPurple,
}

impl Theme {
    /// Every theme, in the order the picker lists them.
    pub const ALL: [Self; 43] = [
        Self::GithubDark,
        Self::GithubLight,
        Self::GithubDarkDimmed,
        Self::GithubHighContrastDark,
        Self::GithubHighContrastLight,
        Self::OneDark,
        Self::OneLight,
        Self::Dracula,
        Self::Monokai,
        Self::MonokaiPro,
        Self::TokyoNight,
        Self::TokyoNightStorm,
        Self::TokyoNightMoon,
        Self::TokyoNightDay,
        Self::CatppuccinMocha,
        Self::CatppuccinMacchiato,
        Self::CatppuccinFrappe,
        Self::CatppuccinLatte,
        Self::Nord,
        Self::GruvboxDark,
        Self::GruvboxLight,
        Self::SolarizedDark,
        Self::SolarizedLight,
        Self::RosePine,
        Self::RosePineMoon,
        Self::RosePineDawn,
        Self::Kanagawa,
        Self::EverforestDark,
        Self::EverforestLight,
        Self::AyuDark,
        Self::AyuMirage,
        Self::AyuLight,
        Self::NightOwl,
        Self::NightOwlLight,
        Self::MaterialOcean,
        Self::Palenight,
        Self::VsCodeDark,
        Self::VsCodeLight,
        Self::Nightfox,
        Self::OceanicNext,
        Self::Cobalt2,
        Self::Synthwave84,
        Self::ShadesOfPurple,
    ];

    pub fn label(self) -> &'static str {
        self.preset().0
    }

    pub const fn colors(self) -> &'static Colors {
        self.preset().1
    }

    const fn preset(self) -> (&'static str, &'static Colors) {
        match self {
            Self::GithubDark => ("GitHub Dark", &GITHUB_DARK),
            Self::GithubLight => ("GitHub Light", &GITHUB_LIGHT),
            Self::GithubDarkDimmed => ("GitHub Dark Dimmed", &GITHUB_DARK_DIMMED),
            Self::GithubHighContrastDark => {
                ("GitHub High Contrast Dark", &GITHUB_HIGH_CONTRAST_DARK)
            }
            Self::GithubHighContrastLight => {
                ("GitHub High Contrast Light", &GITHUB_HIGH_CONTRAST_LIGHT)
            }
            Self::OneDark => ("One Dark", &ONE_DARK),
            Self::OneLight => ("One Light", &ONE_LIGHT),
            Self::Dracula => ("Dracula", &DRACULA),
            Self::Monokai => ("Monokai", &MONOKAI),
            Self::MonokaiPro => ("Monokai Pro", &MONOKAI_PRO),
            Self::TokyoNight => ("Tokyo Night", &TOKYO_NIGHT),
            Self::TokyoNightStorm => ("Tokyo Night Storm", &TOKYO_NIGHT_STORM),
            Self::TokyoNightMoon => ("Tokyo Night Moon", &TOKYO_NIGHT_MOON),
            Self::TokyoNightDay => ("Tokyo Night Day", &TOKYO_NIGHT_DAY),
            Self::CatppuccinMocha => ("Catppuccin Mocha", &CATPPUCCIN_MOCHA),
            Self::CatppuccinMacchiato => ("Catppuccin Macchiato", &CATPPUCCIN_MACCHIATO),
            Self::CatppuccinFrappe => ("Catppuccin Frappé", &CATPPUCCIN_FRAPPE),
            Self::CatppuccinLatte => ("Catppuccin Latte", &CATPPUCCIN_LATTE),
            Self::Nord => ("Nord", &NORD),
            Self::GruvboxDark => ("Gruvbox Dark", &GRUVBOX_DARK),
            Self::GruvboxLight => ("Gruvbox Light", &GRUVBOX_LIGHT),
            Self::SolarizedDark => ("Solarized Dark", &SOLARIZED_DARK),
            Self::SolarizedLight => ("Solarized Light", &SOLARIZED_LIGHT),
            Self::RosePine => ("Rosé Pine", &ROSE_PINE),
            Self::RosePineMoon => ("Rosé Pine Moon", &ROSE_PINE_MOON),
            Self::RosePineDawn => ("Rosé Pine Dawn", &ROSE_PINE_DAWN),
            Self::Kanagawa => ("Kanagawa", &KANAGAWA),
            Self::EverforestDark => ("Everforest Dark", &EVERFOREST_DARK),
            Self::EverforestLight => ("Everforest Light", &EVERFOREST_LIGHT),
            Self::AyuDark => ("Ayu Dark", &AYU_DARK),
            Self::AyuMirage => ("Ayu Mirage", &AYU_MIRAGE),
            Self::AyuLight => ("Ayu Light", &AYU_LIGHT),
            Self::NightOwl => ("Night Owl", &NIGHT_OWL),
            Self::NightOwlLight => ("Night Owl Light", &NIGHT_OWL_LIGHT),
            Self::MaterialOcean => ("Material Ocean", &MATERIAL_OCEAN),
            Self::Palenight => ("Palenight", &PALENIGHT),
            Self::VsCodeDark => ("VS Code Dark", &VS_CODE_DARK),
            Self::VsCodeLight => ("VS Code Light", &VS_CODE_LIGHT),
            Self::Nightfox => ("Nightfox", &NIGHTFOX),
            Self::OceanicNext => ("Oceanic Next", &OCEANIC_NEXT),
            Self::Cobalt2 => ("Cobalt2", &COBALT_2),
            Self::Synthwave84 => ("Synthwave '84", &SYNTHWAVE_84),
            Self::ShadesOfPurple => ("Shades of Purple", &SHADES_OF_PURPLE),
        }
    }
}

/// Every color zdiff draws with.
#[derive(Debug)]
pub struct Colors {
    /// Pane background and plain text; `Reset` keeps the terminal's own.
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub green: Color,
    pub red: Color,
    pub yellow: Color,
    pub bar: Color,
    pub sidebar: Color,
    pub selected: Color,
    pub fold: Color,
    /// Missing side of a line.
    pub filler: Color,
    /// Diagonal stripes, barely above [`Colors::filler`] so they hint rather than distract.
    pub stripe: Color,
    pub modal: Color,
    /// Badges such as ` LINE ` and the footer's repo name.
    pub badge: Color,
    pub badge_fg: Color,
    /// Search matches.
    pub find: Color,
    /// The current search match.
    pub find_current: Color,
    pub added: Color,
    pub removed: Color,
    /// Changed words inside an added or removed line.
    pub added_emph: Color,
    pub removed_emph: Color,
    pub keyword: Color,
    pub string: Color,
    pub constant: Color,
    pub entity: Color,
    pub tag: Color,
    pub comment: Color,
}

impl Colors {
    /// The color of a syntax role.
    pub fn class(&self, class: Class) -> Color {
        match class {
            Class::Keyword => self.keyword,
            Class::String => self.string,
            Class::Constant => self.constant,
            Class::Entity => self.entity,
            Class::Tag => self.tag,
            Class::Comment => self.comment,
        }
    }

    /// The color of a file's status letter.
    pub fn status(&self, status: Status) -> Color {
        match status {
            Status::Added | Status::Untracked => self.green,
            Status::Modified | Status::Renamed => self.yellow,
            Status::Deleted => self.red,
        }
    }
}

const fn hex(rgb: u32) -> Color {
    let [_, r, g, b] = rgb.to_be_bytes();
    Color::Rgb(r, g, b)
}

/// GitHub's dark palette (Primer); diff colors are GitHub's translucent ones blended over `#0d1117`.
static GITHUB_DARK: Colors = Colors {
    bg: Color::Reset,
    fg: Color::Reset,
    dim: hex(0x8c_929e),
    accent: hex(0xe5_c07b),
    green: hex(0x98_c379),
    red: hex(0xe0_6c75),
    yellow: hex(0xe5_c07b),
    bar: hex(0x01_0409),
    sidebar: hex(0x16_181d),
    selected: hex(0x2a_2e38),
    fold: hex(0x20_232a),
    filler: hex(0x16_1b22),
    stripe: hex(0x21_262d),
    modal: hex(0x16_1b22),
    badge: hex(0x1f_6feb),
    badge_fg: Color::Black,
    find: hex(0x5c_4a0f),
    find_current: hex(0x9e_6a03),
    added: hex(0x12_261e),
    removed: hex(0x30_1b1e),
    added_emph: hex(0x1a_4a29),
    removed_emph: hex(0x6b_2b2b),
    keyword: hex(0xff_7b72),
    string: hex(0xa5_d6ff),
    constant: hex(0x79_c0ff),
    entity: hex(0xd2_a8ff),
    tag: hex(0x7e_e787),
    comment: hex(0x91_98a1),
};

/// GitHub's light palette (Primer).
static GITHUB_LIGHT: Colors = Colors {
    bg: hex(0xff_ffff),
    fg: hex(0x1f_2328),
    dim: hex(0x59_636e),
    accent: hex(0x09_69da),
    green: hex(0x1a_7f37),
    red: hex(0xcf_222e),
    yellow: hex(0x9a_6700),
    bar: hex(0xf6_f8fa),
    sidebar: hex(0xf6_f8fa),
    selected: hex(0xe7_ecf0),
    fold: hex(0xef_f2f5),
    filler: hex(0xf6_f8fa),
    stripe: hex(0xe1_e6eb),
    modal: hex(0xff_ffff),
    badge: hex(0x09_69da),
    badge_fg: hex(0xff_ffff),
    find: hex(0xff_f8c5),
    find_current: hex(0xf2_cc60),
    added: hex(0xda_fbe1),
    removed: hex(0xff_ebe9),
    added_emph: hex(0xac_eebb),
    removed_emph: hex(0xff_cecb),
    keyword: hex(0xcf_222e),
    string: hex(0x0a_3069),
    constant: hex(0x05_50ae),
    entity: hex(0x82_50df),
    tag: hex(0x11_6329),
    comment: hex(0x59_636e),
};

/// A theme from its base colors, as `0xRRGGBB` in this order: bg, fg, dim, panel,
/// selection, accent, green, red, yellow, badge, keyword, string, constant, entity,
/// tag, comment. Diff, search, fold, and filler backgrounds are blended from them.
const fn derive(
    [
        bg,
        fg,
        dim,
        panel,
        selection,
        accent,
        green,
        red,
        yellow,
        badge,
        keyword,
        string,
        constant,
        entity,
        tag,
        comment,
    ]: [u32; 16],
) -> Colors {
    Colors {
        bg: hex(bg),
        fg: hex(fg),
        dim: hex(dim),
        accent: hex(accent),
        green: hex(green),
        red: hex(red),
        yellow: hex(yellow),
        bar: hex(panel),
        sidebar: hex(panel),
        selected: hex(selection),
        fold: mix(bg, fg, 6),
        filler: mix(bg, fg, 3),
        stripe: mix(bg, fg, 12),
        modal: hex(panel),
        badge: hex(badge),
        badge_fg: hex(bg),
        find: mix(bg, yellow, 30),
        find_current: mix(bg, yellow, 55),
        added: mix(bg, green, 15),
        removed: mix(bg, red, 15),
        added_emph: mix(bg, green, 35),
        removed_emph: mix(bg, red, 35),
        keyword: hex(keyword),
        string: hex(string),
        constant: hex(constant),
        entity: hex(entity),
        tag: hex(tag),
        comment: hex(comment),
    }
}

/// `a` moved `percent`% of the way toward `b`, per channel.
const fn mix(a: u32, b: u32, percent: u32) -> Color {
    let ([_, ar, ag, ab], [_, br, bg, bb]) = (a.to_be_bytes(), b.to_be_bytes());
    Color::Rgb(
        blend(ar, br, percent),
        blend(ag, bg, percent),
        blend(ab, bb, percent),
    )
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a weighted mean of two u8s fits a u8"
)]
const fn blend(x: u8, y: u8, percent: u32) -> u8 {
    ((x as u32 * (100 - percent) + y as u32 * percent) / 100) as u8
}

static GITHUB_DARK_DIMMED: Colors = derive([
    0x22_272e, 0xad_bac7, 0x76_8390, 0x2d_333b, 0x37_3e47, 0x53_9bf5, 0x57_ab5a, 0xe5_534b,
    0xc6_9026, 0x53_9bf5, 0xf4_7067, 0x96_d0ff, 0x6c_b6ff, 0xdc_bdfb, 0x8d_db8c, 0x76_8390,
]);
static GITHUB_HIGH_CONTRAST_DARK: Colors = derive([
    0x0a_0c10, 0xf0_f3f6, 0x9e_a7b3, 0x27_2b33, 0x52_5964, 0x71_b7ff, 0x26_cd4d, 0xff_6a69,
    0xf0_b72f, 0x40_9eff, 0xff_9492, 0xad_dcff, 0x91_cbff, 0xdb_b7ff, 0x72_f088, 0xbd_c4cc,
]);
static GITHUB_HIGH_CONTRAST_LIGHT: Colors = derive([
    0xff_ffff, 0x0e_1116, 0x66_707b, 0xe7_ecf0, 0xce_dae5, 0x03_49b4, 0x05_5d20, 0xa0_111f,
    0x74_4500, 0x03_49b4, 0xa0_111f, 0x03_2563, 0x02_3b95, 0x62_2cbc, 0x02_4c1a, 0x66_707b,
]);
static ONE_DARK: Colors = derive([
    0x28_2c34, 0xab_b2bf, 0x7f_848e, 0x21_252b, 0x3e_4451, 0x61_afef, 0x98_c379, 0xe0_6c75,
    0xe5_c07b, 0x61_afef, 0xc6_78dd, 0x98_c379, 0xd1_9a66, 0x61_afef, 0xe0_6c75, 0x7f_848e,
]);
static ONE_LIGHT: Colors = derive([
    0xfa_fafa, 0x38_3a42, 0x80_828a, 0xf0_f0f1, 0xe5_e5e6, 0x40_78f2, 0x50_a14f, 0xe4_5649,
    0xc1_8401, 0x40_78f2, 0xa6_26a4, 0x50_a14f, 0x98_6801, 0x40_78f2, 0xe4_5649, 0x80_828a,
]);
static DRACULA: Colors = derive([
    0x28_2a36, 0xf8_f8f2, 0x7f_8ab8, 0x21_222c, 0x44_475a, 0xbd_93f9, 0x50_fa7b, 0xff_5555,
    0xf1_fa8c, 0xbd_93f9, 0xff_79c6, 0xf1_fa8c, 0xbd_93f9, 0x50_fa7b, 0x8b_e9fd, 0x7f_8ab8,
]);
static MONOKAI: Colors = derive([
    0x27_2822, 0xf8_f8f2, 0x90_908a, 0x1e_1f1c, 0x49_483e, 0xfd_971f, 0xa6_e22e, 0xf9_2672,
    0xe6_db74, 0x66_d9ef, 0xf9_2672, 0xe6_db74, 0xae_81ff, 0xa6_e22e, 0x66_d9ef, 0x88_846f,
]);
static MONOKAI_PRO: Colors = derive([
    0x2d_2a2e, 0xfc_fcfa, 0x93_9293, 0x22_1f22, 0x40_3e41, 0xff_d866, 0xa9_dc76, 0xff_6188,
    0xff_d866, 0x78_dce8, 0xff_6188, 0xff_d866, 0xab_9df2, 0xa9_dc76, 0x78_dce8, 0x93_9293,
]);
static TOKYO_NIGHT: Colors = derive([
    0x1a_1b26, 0xc0_caf5, 0x73_7aa2, 0x16_161e, 0x28_3457, 0x7a_a2f7, 0x9e_ce6a, 0xf7_768e,
    0xe0_af68, 0x7a_a2f7, 0xbb_9af7, 0x9e_ce6a, 0xff_9e64, 0x7a_a2f7, 0x7d_cfff, 0x73_7aa2,
]);
static TOKYO_NIGHT_STORM: Colors = derive([
    0x24_283b, 0xc0_caf5, 0x73_7aa2, 0x1f_2335, 0x2e_3c64, 0x7a_a2f7, 0x9e_ce6a, 0xf7_768e,
    0xe0_af68, 0x7a_a2f7, 0xbb_9af7, 0x9e_ce6a, 0xff_9e64, 0x7a_a2f7, 0x7d_cfff, 0x73_7aa2,
]);
static TOKYO_NIGHT_MOON: Colors = derive([
    0x22_2436, 0xc8_d3f5, 0x82_8bb8, 0x1e_2030, 0x2d_3f76, 0x82_aaff, 0xc3_e88d, 0xff_757f,
    0xff_c777, 0x82_aaff, 0xc0_99ff, 0xc3_e88d, 0xff_966c, 0x82_aaff, 0x86_e1fc, 0x82_8bb8,
]);
static TOKYO_NIGHT_DAY: Colors = derive([
    0xe1_e2e7, 0x37_60bf, 0x61_72b0, 0xd0_d5e3, 0xc8_cfe8, 0x2e_7de9, 0x58_7539, 0xf5_2a65,
    0x8c_6c3e, 0x2e_7de9, 0x98_54f1, 0x58_7539, 0xb1_5c00, 0x2e_7de9, 0x00_7197, 0x61_72b0,
]);
static CATPPUCCIN_MOCHA: Colors = derive([
    0x1e_1e2e, 0xcd_d6f4, 0x93_99b2, 0x18_1825, 0x45_475a, 0x89_b4fa, 0xa6_e3a1, 0xf3_8ba8,
    0xf9_e2af, 0x89_b4fa, 0xcb_a6f7, 0xa6_e3a1, 0xfa_b387, 0x89_b4fa, 0x94_e2d5, 0x93_99b2,
]);
static CATPPUCCIN_MACCHIATO: Colors = derive([
    0x24_273a, 0xca_d3f5, 0x93_9ab7, 0x1e_2030, 0x49_4d64, 0x8a_adf4, 0xa6_da95, 0xed_8796,
    0xee_d49f, 0x8a_adf4, 0xc6_a0f6, 0xa6_da95, 0xf5_a97f, 0x8a_adf4, 0x8b_d5ca, 0x93_9ab7,
]);
static CATPPUCCIN_FRAPPE: Colors = derive([
    0x30_3446, 0xc6_d0f5, 0x94_9cbb, 0x29_2c3c, 0x51_576d, 0x8c_aaee, 0xa6_d189, 0xe7_8284,
    0xe5_c890, 0x8c_aaee, 0xca_9ee6, 0xa6_d189, 0xef_9f76, 0x8c_aaee, 0x81_c8be, 0x94_9cbb,
]);
static CATPPUCCIN_LATTE: Colors = derive([
    0xef_f1f5, 0x4c_4f69, 0x6c_6f85, 0xe6_e9ef, 0xcc_d0da, 0x1e_66f5, 0x40_a02b, 0xd2_0f39,
    0xdf_8e1d, 0x1e_66f5, 0x88_39ef, 0x40_a02b, 0xfe_640b, 0x1e_66f5, 0x17_9299, 0x7c_7f93,
]);
static NORD: Colors = derive([
    0x2e_3440, 0xd8_dee9, 0x8a_93a5, 0x3b_4252, 0x43_4c5e, 0x88_c0d0, 0xa3_be8c, 0xbf_616a,
    0xeb_cb8b, 0x81_a1c1, 0x81_a1c1, 0xa3_be8c, 0xb4_8ead, 0x88_c0d0, 0x8f_bcbb, 0x7b_88a1,
]);
static GRUVBOX_DARK: Colors = derive([
    0x28_2828, 0xeb_dbb2, 0xa8_9984, 0x3c_3836, 0x50_4945, 0xfa_bd2f, 0xb8_bb26, 0xfb_4934,
    0xfa_bd2f, 0x83_a598, 0xfb_4934, 0xb8_bb26, 0xd3_869b, 0xfa_bd2f, 0x8e_c07c, 0x92_8374,
]);
static GRUVBOX_LIGHT: Colors = derive([
    0xfb_f1c7, 0x3c_3836, 0x7c_6f64, 0xeb_dbb2, 0xd5_c4a1, 0xb5_7614, 0x79_740e, 0x9d_0006,
    0xb5_7614, 0x07_6678, 0x9d_0006, 0x79_740e, 0x8f_3f71, 0xb5_7614, 0x42_7b58, 0x92_8374,
]);
static SOLARIZED_DARK: Colors = derive([
    0x00_2b36, 0x83_9496, 0x75_898f, 0x07_3642, 0x0a_3f4c, 0x26_8bd2, 0x85_9900, 0xdc_322f,
    0xb5_8900, 0x26_8bd2, 0x85_9900, 0x2a_a198, 0xd3_3682, 0x26_8bd2, 0xcb_4b16, 0x65_7b83,
]);
static SOLARIZED_LIGHT: Colors = derive([
    0xfd_f6e3, 0x58_6e75, 0x6c_7f86, 0xee_e8d5, 0xe4_ddc8, 0x26_8bd2, 0x85_9900, 0xdc_322f,
    0xb5_8900, 0x26_8bd2, 0x85_9900, 0x2a_a198, 0xd3_3682, 0x26_8bd2, 0xcb_4b16, 0x93_a1a1,
]);
static ROSE_PINE: Colors = derive([
    0x19_1724, 0xe0_def4, 0x90_8caa, 0x1f_1d2e, 0x40_3d52, 0xc4_a7e7, 0x9c_cfd8, 0xeb_6f92,
    0xf6_c177, 0xc4_a7e7, 0x31_748f, 0xf6_c177, 0xeb_6f92, 0xeb_bcba, 0x9c_cfd8, 0x6e_6a86,
]);
static ROSE_PINE_MOON: Colors = derive([
    0x23_2136, 0xe0_def4, 0x90_8caa, 0x2a_273f, 0x44_415a, 0xc4_a7e7, 0x9c_cfd8, 0xeb_6f92,
    0xf6_c177, 0xc4_a7e7, 0x3e_8fb0, 0xf6_c177, 0xeb_6f92, 0xea_9a97, 0x9c_cfd8, 0x6e_6a86,
]);
static ROSE_PINE_DAWN: Colors = derive([
    0xfa_f4ed, 0x57_5279, 0x79_7593, 0xf2_e9e1, 0xdf_dad9, 0x90_7aa9, 0x56_949f, 0xb4_637a,
    0xea_9d34, 0x90_7aa9, 0x28_6983, 0xea_9d34, 0xb4_637a, 0xd7_827e, 0x56_949f, 0x98_93a5,
]);
static KANAGAWA: Colors = derive([
    0x1f_1f28, 0xdc_d7ba, 0x93_8aa9, 0x16_161d, 0x2d_4f67, 0x7e_9cd8, 0x98_bb6c, 0xe4_6876,
    0xe6_c384, 0x7e_9cd8, 0x95_7fb8, 0x98_bb6c, 0xff_a066, 0x7e_9cd8, 0x7a_a89f, 0x72_7169,
]);
static EVERFOREST_DARK: Colors = derive([
    0x2d_353b, 0xd3_c6aa, 0x9d_a9a0, 0x34_3f44, 0x47_5258, 0xa7_c080, 0xa7_c080, 0xe6_7e80,
    0xdb_bc7f, 0x7f_bbb3, 0xe6_7e80, 0xa7_c080, 0xd6_99b6, 0x83_c092, 0x7f_bbb3, 0x85_9289,
]);
static EVERFOREST_LIGHT: Colors = derive([
    0xfd_f6e3, 0x5c_6a72, 0x70_8070, 0xf4_f0d9, 0xe6_e2cc, 0x2f_83b0, 0x8d_a101, 0xf8_5552,
    0xdf_a000, 0x3a_94c5, 0xf8_5552, 0x8d_a101, 0xdf_69ba, 0x35_a77c, 0x3a_94c5, 0x93_9f91,
]);
static AYU_DARK: Colors = derive([
    0x0d_1017, 0xbf_bdb6, 0x6c_7380, 0x13_1721, 0x27_3747, 0xe6_b450, 0x7f_d962, 0xf2_6d78,
    0xe6_b450, 0x59_c2ff, 0xff_8f40, 0xaa_d94c, 0xd2_a6ff, 0xff_b454, 0x39_bae6, 0x62_6a73,
]);
static AYU_MIRAGE: Colors = derive([
    0x1f_2430, 0xcc_cac2, 0x8a_9199, 0x23_2834, 0x33_415e, 0xff_cc66, 0x87_d96c, 0xf2_7983,
    0xff_cc66, 0x73_d0ff, 0xff_ad66, 0xd5_ff80, 0xdf_bfff, 0xff_d173, 0x5c_cfe6, 0x6e_7c8f,
]);
static AYU_LIGHT: Colors = derive([
    0xfc_fcfc, 0x5c_6166, 0x78_7b80, 0xf3_f4f5, 0xd3_e1f5, 0xd4_691a, 0x6c_bf43, 0xe6_5050,
    0xf2_ae49, 0x3a_8ad0, 0xfa_8d3e, 0x86_b300, 0xa3_7acc, 0xf2_ae49, 0x55_b4d4, 0xa0_a3a8,
]);
static NIGHT_OWL: Colors = derive([
    0x01_1627, 0xd6_deeb, 0x7e_97a8, 0x0b_2942, 0x1d_3b53, 0x82_aaff, 0xad_db67, 0xef_5350,
    0xec_c48d, 0x82_aaff, 0xc7_92ea, 0xec_c48d, 0xf7_8c6c, 0x82_aaff, 0x7f_dbca, 0x63_7777,
]);
static NIGHT_OWL_LIGHT: Colors = derive([
    0xfb_fbfb, 0x40_3f53, 0x6a_6d82, 0xf0_f0f0, 0xe0_e0e0, 0x48_76d6, 0x08_916a, 0xde_3d3b,
    0xda_aa01, 0x48_76d6, 0x99_4cc3, 0xc9_6765, 0xaa_0982, 0x48_76d6, 0x0c_969b, 0x98_9fb1,
]);
static MATERIAL_OCEAN: Colors = derive([
    0x0f_111a, 0xa6_accd, 0x71_7cb4, 0x09_0b10, 0x23_2637, 0x84_ffff, 0xc3_e88d, 0xf0_7178,
    0xff_cb6b, 0x82_aaff, 0xc7_92ea, 0xc3_e88d, 0xf7_8c6c, 0x82_aaff, 0x89_ddff, 0x46_4b5d,
]);
static PALENIGHT: Colors = derive([
    0x29_2d3e, 0xa6_accd, 0x7e_86b0, 0x20_2331, 0x44_4267, 0xc7_92ea, 0xc3_e88d, 0xf0_7178,
    0xff_cb6b, 0x82_aaff, 0xc7_92ea, 0xc3_e88d, 0xf7_8c6c, 0x82_aaff, 0x89_ddff, 0x67_6e95,
]);
static VS_CODE_DARK: Colors = derive([
    0x1e_1e1e, 0xd4_d4d4, 0x9d_9d9d, 0x25_2526, 0x37_373d, 0x37_94ff, 0x81_b88b, 0xf1_4c4c,
    0xcc_a700, 0x00_78d4, 0x56_9cd6, 0xce_9178, 0xb5_cea8, 0xdc_dcaa, 0x4e_c9b0, 0x6a_9955,
]);
static VS_CODE_LIGHT: Colors = derive([
    0xff_ffff, 0x1f_1f1f, 0x61_6161, 0xf3_f3f3, 0xe4_e6f1, 0x00_5fb8, 0x38_8a34, 0xcd_3131,
    0xbf_8803, 0x00_5fb8, 0x00_00ff, 0xa3_1515, 0x09_8658, 0x79_5e26, 0x26_7f99, 0x00_8000,
]);
static NIGHTFOX: Colors = derive([
    0x19_2330, 0xcd_cecf, 0x87_94a8, 0x13_1a24, 0x2b_3b51, 0x71_9cd6, 0x81_b29a, 0xc9_4f6d,
    0xdb_c074, 0x71_9cd6, 0x9d_79d6, 0x81_b29a, 0xf4_a261, 0x71_9cd6, 0x63_cdcf, 0x73_8091,
]);
static OCEANIC_NEXT: Colors = derive([
    0x1b_2b34, 0xd8_dee9, 0x89_95a1, 0x34_3d46, 0x4f_5b66, 0x66_99cc, 0x99_c794, 0xec_5f67,
    0xfa_c863, 0x66_99cc, 0xc5_94c5, 0x99_c794, 0xf9_9157, 0x66_99cc, 0x5f_b3b3, 0x65_737e,
]);
static COBALT_2: Colors = derive([
    0x19_3549, 0xff_ffff, 0x8f_a3b3, 0x15_232d, 0x1f_4662, 0xff_c600, 0x3a_d900, 0xff_628c,
    0xff_c600, 0x00_88ff, 0xff_9d00, 0xa5_ff90, 0xff_628c, 0xff_c600, 0x9e_ffff, 0x00_88ff,
]);
static SYNTHWAVE_84: Colors = derive([
    0x26_2335, 0xf0_eff1, 0xa8_a2c6, 0x24_1b2f, 0x46_3465, 0xff_7edb, 0x72_f1b8, 0xfe_4450,
    0xfe_de5d, 0x36_f9f6, 0xfe_de5d, 0xff_8b39, 0xf9_7e72, 0x36_f9f6, 0x72_f1b8, 0x84_8bbd,
]);
static SHADES_OF_PURPLE: Colors = derive([
    0x2d_2b55, 0xff_ffff, 0xb8_b0f0, 0x1e_1e3f, 0x4d_3f8f, 0xfa_d000, 0x3a_d900, 0xec_3a37,
    0xfa_d000, 0x9e_ffff, 0xff_9d00, 0xa5_ff90, 0xff_628c, 0xfa_d000, 0x9e_ffff, 0xb3_62ff,
]);

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio of two colors; `None` for the terminal's own.
    fn contrast(first: Color, second: Color) -> Option<f64> {
        let luminance = |color| {
            let Color::Rgb(red, green, blue) = color else {
                return None;
            };
            let linear = |c: u8| {
                let c = f64::from(c) / 255.0;
                if c <= 0.039_28 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            Some(0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue))
        };
        let (first, second) = (luminance(first)?, luminance(second)?);
        Some((first.max(second) + 0.05) / (first.min(second) + 0.05))
    }

    #[test]
    fn every_theme_has_a_unique_label_and_round_trips_by_name() {
        let mut labels: Vec<_> = Theme::ALL.iter().map(|t| t.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), Theme::ALL.len());
        for theme in Theme::ALL {
            let name = toml::Value::try_from(theme).expect("serializes");
            assert_eq!(name.try_into::<Theme>().ok(), Some(theme));
        }
    }

    #[test]
    fn every_theme_keeps_its_text_readable() {
        for theme in Theme::ALL {
            let c = theme.colors();
            let pairs = [
                ("text", c.fg, c.bg, 4.5),
                ("dim text", c.dim, c.bg, 3.0),
                ("dim text on panels", c.dim, c.modal, 3.0),
                ("selected row", c.fg, c.selected, 3.5),
                ("accent", c.accent, c.bg, 3.0),
                ("badge", c.badge_fg, c.badge, 3.0),
            ];
            for (what, fg, bg, floor) in pairs {
                if let Some(ratio) = contrast(fg, bg) {
                    assert!(ratio >= floor, "{}: {what} is {ratio:.2}", theme.label());
                }
            }
        }
    }
}
