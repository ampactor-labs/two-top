//! Every arena's presentation, in one table.
//!
//! What an arena is called (three ways, for three places), the floor it
//! stands on, its join-link tag, the rule line the roster shows, the skin
//! its cover wears and the air drifting over it. The title screen, the
//! roster, the intro card, the replay list, room codes, the floor, the
//! cover and the ambient motes all read [`info`], so a visuals pass on an
//! arena changes it here and every screen, on every build, follows.
//!
//! Not here: the arena's *rules* (geometry, pyres, bridges, doors). Those
//! are the sim's (`sim::arena_*`), and changing them is a `SIM_VERSION`
//! bump, not a restyle.

use bevy::prelude::Color;
use sim::ArenaId;

use crate::palette as pal;

/// One arena's presentation.
#[derive(Clone, Copy, Debug)]
pub struct ArenaInfo {
    /// Its name in capitals, as the roster and the intro card print it
    /// ("THE PIT").
    pub title: &'static str,
    /// Its name in title case, for the title screen's picker ("The Pit").
    pub name: &'static str,
    /// Its short label, for the replay list ("PIT").
    pub label: &'static str,
    /// Its join-link token (`join.html#CURS-pit`). Links already shared
    /// depend on it: never rename one.
    pub room_tag: &'static str,
    /// The composed floor texture (DESIGN_DIRECTION § 4: one quiet shared
    /// composition retinted per arena; props carry the rest).
    pub floor: &'static str,
    /// One line of the rules that make the arena itself. The roster is
    /// where a player learns the roster's verbs, so it says mechanics, not
    /// mood.
    pub rule: &'static str,
    /// The raised cover's skin.
    pub cover: CoverSkin,
    /// The ambient air: motes spawned per second, and the sheet row (0
    /// ember, 1 cold dust, 2 grove spore). The air says where you are
    /// before the floor does.
    pub air: (f32, u16),
}

/// Per-arena cover skin: the front-face and lid colors plus which motif
/// the lid carries. Flat quads still sell the 2.5D read; identity comes
/// from the register and one quiet motif per arena, so a crate, a pillar,
/// a reliquary bar, a pit stone and a museum plinth stop being the same
/// gray box everywhere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverSkin {
    pub front: Color,
    pub top: Color,
    pub motif: CoverMotif,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoverMotif {
    /// Anchor: rope lashing across the crate lid (an X of thin straps).
    Lashing,
    /// Crossing: two vertical flutes carved into the pillar's front face.
    Flutes,
    /// Reliquary: the sealed teal seam along the bar's long axis.
    Seam,
    /// The Pit: an ember crack — the stone remembers the strikes.
    Crack,
    /// The Gallery: the lit bone rim on the plinth's near lid edge.
    PlinthRim,
}

/// The presentation of `id`.
pub fn info(id: ArenaId) -> &'static ArenaInfo {
    match id {
        ArenaId::Anchor => &ANCHOR,
        ArenaId::Crossing => &CROSSING,
        ArenaId::Reliquary => &RELIQUARY,
        ArenaId::Pit => &PIT,
        ArenaId::Vigil => &VIGIL,
        ArenaId::Gallery => &GALLERY,
        ArenaId::Forest => &FOREST,
    }
}

/// No cover stands on these rosters today; a sane skin if one lands.
const PLAIN_COVER: CoverSkin = CoverSkin {
    front: pal::CHARCOAL_LINE,
    top: pal::COLD_STONE,
    motif: CoverMotif::PlinthRim,
};

const ANCHOR: ArenaInfo = ArenaInfo {
    title: "ANCHOR",
    name: "Anchor",
    label: "ANCHOR",
    room_tag: "anchor",
    floor: "arenas/anchor_floor.png",
    rule: "four crates - one pyre",
    cover: CoverSkin {
        front: pal::BRUISE_SHADOW,
        top: pal::WARM_BONE_SHADE,
        motif: CoverMotif::Lashing,
    },
    air: (4.0, 0),
};

const CROSSING: ArenaInfo = ArenaInfo {
    title: "CROSSING",
    name: "Crossing",
    label: "CROSSING",
    room_tag: "crossing",
    floor: "arenas/crossing_floor.png",
    rule: "a moat between you - ring a sigil to bridge it",
    cover: CoverSkin {
        front: pal::DEEP_ASH,
        top: pal::COLD_STONE,
        motif: CoverMotif::Flutes,
    },
    air: (2.5, 1),
};

const RELIQUARY: ArenaInfo = ArenaInfo {
    title: "RELIQUARY",
    name: "Reliquary",
    label: "RELIQUARY",
    room_tag: "reliquary",
    floor: "arenas/reliquary_floor.png",
    rule: "doors teleport - pyres chain",
    cover: CoverSkin {
        front: pal::BRUISE_SHADOW,
        top: pal::WARM_BONE_SHADE,
        motif: CoverMotif::Seam,
    },
    air: (3.0, 2),
};

const PIT: ArenaInfo = ArenaInfo {
    title: "THE PIT",
    name: "The Pit",
    label: "PIT",
    room_tag: "pit",
    floor: "arenas/pit_floor.png",
    rule: "walls ricochet - nothing falls",
    cover: CoverSkin {
        front: pal::CHARCOAL_LINE,
        top: pal::BRUISE_SHADOW,
        motif: CoverMotif::Crack,
    },
    air: (8.0, 0),
};

const VIGIL: ArenaInfo = ArenaInfo {
    title: "THE VIGIL",
    name: "The Vigil",
    label: "VIGIL",
    room_tag: "vigil",
    floor: "arenas/vigil_floor.png",
    rule: "no storm - open ground",
    cover: PLAIN_COVER,
    air: (1.2, 1),
};

const GALLERY: ArenaInfo = ArenaInfo {
    title: "THE GALLERY",
    name: "The Gallery",
    label: "GALLERY",
    room_tag: "gallery",
    floor: "arenas/gallery_floor.png",
    rule: "corridors of angles",
    cover: CoverSkin {
        front: pal::DEEP_ASH,
        top: pal::COLD_STONE,
        motif: CoverMotif::PlinthRim,
    },
    air: (1.8, 1),
};

const FOREST: ArenaInfo = ArenaInfo {
    title: "THE FOREST",
    name: "The Forest",
    label: "FOREST",
    room_tag: "forest",
    floor: "arenas/forest_floor.png",
    rule: "the grove burns",
    cover: PLAIN_COVER,
    air: (5.0, 2),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_arena_is_named_and_its_join_tag_is_one_clean_unique_token() {
        let mut tags = std::collections::BTreeSet::new();
        for id in sim::ALL_ARENAS {
            let a = info(id);
            assert!(!a.title.is_empty() && !a.name.is_empty() && !a.label.is_empty());
            assert_eq!(a.title, a.title.to_ascii_uppercase(), "{}", a.title);
            assert!(
                a.room_tag
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "{}",
                a.room_tag
            );
            assert!(tags.insert(a.room_tag), "{} twice", a.room_tag);
            assert!(a.floor.starts_with("arenas/") && a.floor.ends_with(".png"));
        }
    }

    #[test]
    fn every_rule_line_fits_the_roster_row_in_the_bundled_font() {
        for id in sim::ALL_ARENAS {
            let a = info(id);
            assert!(
                a.rule.len() < 48,
                "{:?} is too long for a roster row",
                a.rule
            );
            for text in [a.title, a.name, a.label, a.rule] {
                assert!(
                    text.chars().all(|c| c.is_ascii() && c != '\u{2014}'),
                    "{text:?}: ASCII only, the bundled font renders tofu otherwise"
                );
            }
        }
    }

    /// Join links already shared carry these: renaming one strands them.
    #[test]
    fn the_join_tags_never_change() {
        let tags: Vec<_> = sim::ALL_ARENAS.iter().map(|&a| info(a).room_tag).collect();
        assert_eq!(
            tags,
            [
                "anchor",
                "crossing",
                "reliquary",
                "pit",
                "vigil",
                "gallery",
                "forest"
            ]
        );
    }
}
