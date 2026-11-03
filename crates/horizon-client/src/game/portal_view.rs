//! The levels behind portals, for seeing through them (Immersive Portals' view) where this
//! game holds the world: single player and a LAN host, whose integrated server runs every level.
//! The window asks for the level behind each portal near the player, and behind the portals seen
//! in those levels (`request_portal_view`); the server, when it ticks a level, finds each exit
//! portal as `NetherPortalBlock.getExitPortal` would without building one, keeps its chunks
//! running with portal tickets, and publishes them (`PortalSnapshot`). A server the game only
//! joins sends nothing of another level, so there portals stay vanilla's.
use std::collections::VecDeque;
use super::{Dim, Game, end::END_SPAWN};
use horizon_client::chunk::Chunk;
use horizon_client::fast_hash::{FxHashMap, FxHashSet};
use horizon_client::gfx::{MAX_PORTAL_VIEWS, PORTAL_X};
use horizon_client::mesher::{SectionMesh, Vertex};
use horizon_core::geometry::Axis;
use horizon_core::{geometry::Vec3, position::BlockPos};
use horizon_world::level::portal_travel::{
    FoundRectangle, closest_portal, largest_rectangle_around,
};
use horizon_world::registry::BlockStateId;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// How far from the player or from an exit a portal is seen through (blocks).
const RANGE: i32 = 48;
/// How far from the player a portal stays drawn, plain, past that.
const PLAIN_RANGE: i32 = 128;
/// How many portals deep a view goes: 1 sees through the portals in the window only.
const DEPTH: usize = horizon_client::gfx::MAX_PORTAL_VIEWS;
/// How many chunks around the exit go into a snapshot, each way.
pub(super) const VIEW_CHUNKS: i32 = 8;
/// How soon a column seen through the portal is meshed again after a change: the server's light
/// keeps settling a few values a tick, which would mesh some column every frame.
const REMESH_AFTER: Duration = Duration::from_millis(500);
/// How often the server looks for an exit again while the request stays the same (ticks).
const SEARCH_EVERY: u64 = 40;
const LEVELS: [Dim; 3] = [Dim::Overworld, Dim::Nether, Dim::End];

/// A level's index among the renderer's regions and `Streaming::mesh_foreign`'s tags.
fn region_of(dim: Dim) -> usize {
    match dim {
        Dim::Overworld => 0,
        Dim::Nether => 1,
        Dim::End => 2,
    }
}
/// Where the level's chunks sit in the renderer: their x plus this.
fn region_x(dim: Dim) -> i32 {
    PORTAL_X * (region_of(dim) as i32 + 1)
}

/// The level behind a portal the window wants: where it leads and where its exit is looked for
/// (`Plan.approximate`: the border-clamped, scaled position).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PortalRequest {
    /// The portal asked about: its level and low corner. Its `approximate` moves with the player
    /// and its snapshot stays until the next one.
    pub(super) from: (Dim, BlockPos),
    pub(super) to: Dim,
    pub(super) approximate: BlockPos,
    /// An End portal's blocks, whose crossing lands where the view showed
    /// (`portal_view_landing`).
    pub(super) entry: Option<FoundRectangle>,
    /// The crossing takes the level over (`native_portal_view`'s `crossing`).
    pub(super) crossing: bool,
}

/// A portal the window sees through, directly or in another's view.
#[derive(Clone, Copy, Debug)]
pub(super) struct PortalEntry {
    /// Its blocks and, for a Nether portal, whether it spans the x axis (None: an End portal,
    /// flat).
    area: FoundRectangle,
    x_axis: Option<bool>,
    request: PortalRequest,
    /// The entry whose view shows this portal (earlier in the list), None in the window.
    parent: Option<usize>,
    depth: usize,
}

/// The level behind the portal, as the server held it on its last tick there.
#[derive(Clone)]
pub(super) struct PortalSnapshot {
    pub(super) request: PortalRequest,
    /// The exit portal's blocks, `None` while there is none (one is built on the first crossing).
    pub(super) exit: Option<FoundRectangle>,
    /// Whether the exit portal spans the x axis.
    pub(super) exit_x_axis: bool,
    /// The chunks around the exit (or the approximate position), shared with the server.
    pub(super) chunks: Vec<((i32, i32), Arc<Chunk>)>,
    pub(super) bottom_y: i32,
    /// The level's entities as this tick left them.
    pub(super) entities: Arc<PortalEntities>,
}

/// A level's entities, as the server holds them, for the window to draw in the views into it:
/// the level is loaded behind the portal, so what is in it is known, players who crossed included.
#[derive(Clone, Default)]
pub(super) struct PortalEntities {
    mobs: Vec<horizon_client::mobs::Mob>,
    dropped: Vec<horizon_client::entities::ItemEntity>,
    armor_stands: Vec<horizon_client::entities::ArmorStand>,
    boats: Vec<super::boats::Boat>,
    carts: Vec<super::minecarts::Minecart>,
    /// The LAN players in the level: the window makes their mobs, the server has none.
    players: Vec<PortalPlayer>,
}

/// A LAN player in a level behind a portal.
#[derive(Clone, Copy)]
pub(super) struct PortalPlayer {
    pub(super) id: u32,
    pub(super) uuid: u128,
    pub(super) position: Vec3,
    pub(super) previous: Vec3,
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) head_yaw: f32,
}

/// The server's last search for an exit: for which request, what it found and when.
#[derive(Clone, Copy, Debug)]
pub(super) struct ExitSearch {
    request: PortalRequest,
    exit: Option<(FoundRectangle, bool)>,
    at: u64,
}

/// A level behind portals as the renderer holds it.
#[derive(Default)]
struct PortalRegion {
    /// The columns meshed, as they were then, and when.
    meshed: FxHashMap<(i32, i32), (Arc<Chunk>, Instant)>,
    uploaded: usize,
    logged: bool,
    /// The latest mesh of each of its sections (the chunks' own keys) and the bottom of its
    /// chunk, kept for the crossing (`mesh_portal_view`).
    cache: FxHashMap<(i32, i32, i32), (i32, SectionMesh)>,
    /// The level's entities the window draws, from which list of the server's, and its chunks,
    /// where they read their light (`portal_entity_vertices`).
    entities: PortalEntities,
    entities_from: Option<Arc<PortalEntities>>,
    light: horizon_client::chunk_map::ChunkMap,
}

/// The levels behind portals as the renderer holds them.
#[derive(Default)]
pub(super) struct PortalMeshes {
    /// `Renderer::sections_cleared` when these were uploaded.
    cleared: u64,
    regions: [PortalRegion; LEVELS.len()],
}

/// How long the meshes a crossing took over may stand in for chunks the server does not send.
const STAND_IN_FOR: Duration = Duration::from_secs(20);
/// A crossing's stand-ins go up nearest first, for this long each frame.
const STAND_IN_BUDGET: Duration = Duration::from_millis(3);
/// The meshes of the levels behind portals go up for this long each frame.
const FOREIGN_BUDGET: Duration = Duration::from_millis(2);

/// The portal the camera is in: where a crossing takes what is at its entry.
#[derive(Clone, Copy)]
pub(super) struct PortalHop {
    to_level: Dim,
    from: glam::DVec3,
    to: glam::DVec3,
    turn: f64,
}

/// A crossing's landing: where the player goes on from, and when it crossed.
pub(super) struct PortalJump {
    position: Vec3,
    velocity: Vec3,
    at: Instant,
}

/// The portals within `RANGE` of `around`, nearest first: `kind` tells a portal's blocks, by
/// whether they span the x axis (None: an End portal, flat).
fn portal_rectangles<'a>(
    chunk: impl Fn((i32, i32)) -> Option<&'a Chunk>,
    around: [f64; 3],
    range: i32,
    kind: impl Fn(BlockStateId) -> Option<Option<bool>>,
) -> Vec<(FoundRectangle, Option<bool>)> {
    let at = around.map(|v| v.floor() as i32);
    let mut found: Vec<(FoundRectangle, Option<bool>)> = Vec::new();
    let inside = |(a, x_axis): &(FoundRectangle, Option<bool>), p: [i32; 3]| {
        let m = a.min_corner;
        let (w, h) = (a.axis1_size, a.axis2_size);
        let (dx, dy, dz) = match x_axis {
            Some(true) => (w, h, 1),
            Some(false) => (1, h, w),
            None => (w, 1, h),
        };
        (m.x..m.x + dx).contains(&p[0])
            && (m.y..m.y + dy).contains(&p[1])
            && (m.z..m.z + dz).contains(&p[2])
    };
    for cz in (at[2] - range) >> 4..=(at[2] + range) >> 4 {
        for cx in (at[0] - range) >> 4..=(at[0] + range) >> 4 {
            let Some(column) = chunk((cx, cz)) else {
                continue;
            };
            for (s, section) in column.sections.iter().enumerate() {
                let base = column.min_y() + 16 * s as i32;
                if base > at[1] + range
                    || base + 16 < at[1] - range
                    || !section.blocks.maybe_has(|v| kind(BlockStateId(v)).is_some())
                {
                    continue;
                }
                for y in base..base + 16 {
                    for z in cz * 16..cz * 16 + 16 {
                        for x in cx * 16..cx * 16 + 16 {
                            let state = column.block(x, y, z);
                            let Some(x_axis) = kind(state) else {
                                continue;
                            };
                            if found.iter().any(|f| inside(f, [x, y, z])) {
                                continue;
                            }
                            let same = |p: BlockPos| {
                                chunk((p.x >> 4, p.z >> 4))
                                    .is_some_and(|c| c.block(p.x, p.y, p.z) == state)
                            };
                            let start = BlockPos::new(x, y, z);
                            let area = match x_axis {
                                Some(true) => {
                                    largest_rectangle_around(start, Axis::X, 21, Axis::Y, 21, same)
                                }
                                Some(false) => {
                                    largest_rectangle_around(start, Axis::Z, 21, Axis::Y, 21, same)
                                }
                                None => {
                                    largest_rectangle_around(start, Axis::X, 21, Axis::Z, 21, same)
                                }
                            };
                            found.push((area, x_axis));
                        }
                    }
                }
            }
        }
    }
    let distance = |a: &FoundRectangle| {
        let m = a.min_corner;
        [m.x, m.y, m.z]
            .iter()
            .zip(around)
            .map(|(c, e)| (f64::from(*c) - e).powi(2))
            .sum::<f64>()
    };
    found.retain(|(a, _)| distance(a) <= f64::from(range * range) * 2.0);
    found.sort_by(|a, b| distance(&a.0).total_cmp(&distance(&b.0)));
    found
}

/// How far inside the frame's far face the opening lies, in blocks.
const PORTAL_OPENING_INSET: f64 = 1.0 / 128.0;

/// A portal's blocks as a box (min, max) a hair larger, `shift` blocks east.
fn portal_box(a: &FoundRectangle, x_axis: bool, shift: f64) -> [glam::DVec3; 2] {
    use glam::DVec3;
    let m = a.min_corner;
    let (w, h) = (f64::from(a.axis1_size), f64::from(a.axis2_size));
    let min = DVec3::new(f64::from(m.x) + shift, f64::from(m.y), f64::from(m.z));
    let size = if x_axis {
        DVec3::new(w, h, 1.0)
    } else {
        DVec3::new(1.0, h, w)
    };
    [min - 0.01, min + size + 0.01]
}

impl Game<'_> {
    /// Forgets a level behind portals in the renderer and in the streaming.
    fn drop_portal_region(&mut self, i: usize) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let region = std::mem::take(&mut self.portal_meshes.regions[i]);
        let east = region_x(LEVELS[i]);
        let (stale, kept): (Vec<_>, _) = std::mem::take(&mut self.portal_meshed)
            .into_iter()
            .partition(|(tag, _, _)| usize::from(*tag) == i);
        self.portal_meshed = VecDeque::from(kept);
        self.streaming
            .free(stale.into_iter().map(|(_, key, mesh)| (key, mesh)).collect());
        horizon_client::spans::time("portal_drop", || {
            renderer.remove_columns(region.meshed.keys().map(|&(x, z)| (x + east, z)));
        });
        self.streaming.drop_foreign(i as u8);
        self.streaming.free(
            region
                .cache
                .into_iter()
                .map(|(k, (_, mesh))| (k, mesh))
                .collect(),
        );
    }

    /// The window's side, each frame: the chunks of the levels behind portals meshed by the
    /// streaming workers and uploaded `region_x` east, again when the server changes one.
    pub(super) fn mesh_portal_view(&mut self) {
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let views = self
            .integrated
            .as_ref()
            .map_or_else(Vec::new, |s| s.portal_views());
        let cleared = renderer.sections_cleared();
        // The crossing: the window now holds a level it saw through a portal (its sections
        // were all cleared). What it saw stands in, in place, until the streaming meshes the
        // chunks the server sends: no wait on an empty world.
        let here = region_of(self.dim);
        if cleared != self.portal_meshes.cleared
            && self.portal_view_on.1
            && !self.portal_meshes.regions[here].cache.is_empty()
        {
            let bottom = self.world.bottom_y();
            let sections = self.world.layout().sections() as i32;
            let centre = self
                .portal_jump
                .as_ref()
                .map_or(self.previous_position, |j| j.position);
            let distance = |x: i32, y: i32, z: i32| {
                let d = |c: f64, s: i32| c - f64::from(s) * 16.0 - 8.0;
                let (dx, dy, dz) = (d(centre.x, x), d(centre.y - f64::from(bottom), y), d(centre.z, z));
                dx * dx + dy * dy + dz * dz
            };
            let mut columns = FxHashSet::default();
            let mut waiting = Vec::new();
            let mut spent = Vec::new();
            for ((x, sy, z), (min_y, mesh)) in
                std::mem::take(&mut self.portal_meshes.regions[here].cache)
            {
                let key = (x, sy + (min_y - bottom).div_euclid(16), z);
                if (0..sections).contains(&key.1) && !renderer.has_section(key) {
                    columns.insert((x, z));
                    waiting.push((key, (x, sy, z), mesh));
                } else {
                    spent.push(((x, sy, z), mesh));
                }
            }
            self.streaming.free(spent);
            // Farthest first: the nearest are taken off the end.
            waiting.sort_by(|a, b| {
                distance(b.0.0, b.0.1, b.0.2).total_cmp(&distance(a.0.0, a.0.1, a.0.2))
            });
            log::info!(
                "Oxyra: crossed into {:?} with {} columns already meshed",
                self.dim,
                columns.len()
            );
            self.portal_stand_ins = waiting;
            self.portal_crossed = Some((Instant::now(), columns));
        }
        if !self.portal_stand_ins.is_empty() {
            let started = Instant::now();
            let mut spent = Vec::new();
            while started.elapsed() < STAND_IN_BUDGET {
                let Some((key, cached, mesh)) = self.portal_stand_ins.pop() else {
                    break;
                };
                if !renderer.has_section(key) {
                    renderer.upload(key, &mesh);
                }
                spent.push((cached, mesh));
            }
            self.streaming.free(spent);
        }
        // Stand-ins the server's chunks replace are left to the streaming; the others go.
        if let Some((at, columns)) = &mut self.portal_crossed {
            columns.retain(|c| !self.world.chunks.contains_key(c));
            if at.elapsed() > STAND_IN_FOR {
                renderer.remove_columns(columns.iter().copied());
                self.portal_crossed = None;
            }
        }
        let was_cleared = cleared != self.portal_meshes.cleared;
        self.portal_meshes.cleared = cleared;
        let now = Instant::now();
        for (i, &dim) in LEVELS.iter().enumerate() {
            let wanted: Vec<_> = views.iter().filter(|v| v.request.to == dim).collect();
            if was_cleared || wanted.is_empty() {
                let region = &self.portal_meshes.regions[i];
                if !region.meshed.is_empty() || !region.cache.is_empty() {
                    self.drop_portal_region(i);
                }
                if wanted.is_empty() {
                    continue;
                }
            }
            let Some(renderer) = self.renderer.as_mut() else {
                return;
            };
            let chunks: FxHashMap<_, _> = wanted
                .iter()
                .flat_map(|v| v.chunks.iter().cloned())
                .collect();
            let east = region_x(dim);
            let region = &mut self.portal_meshes.regions[i];
            let gone: Vec<_> = region
                .meshed
                .keys()
                .filter(|k| !chunks.contains_key(k))
                .copied()
                .collect();
            for k in &gone {
                region.meshed.remove(k);
            }
            renderer.remove_columns(gone.iter().map(|&(x, z)| (x + east, z)));
            for (&(x, z), chunk) in &chunks {
                // The server copies a chunk each tick it runs; its sections stay shared until
                // set.
                // shortcut: only the changed sections and the two beside them are meshed again,
                // not the neighbouring columns' (their border faces and light lag a little).
                let sections: Vec<i32> = match region.meshed.get(&(x, z)) {
                    Some((old, _)) if old.sections.len() == chunk.sections.len() => {
                        let n = chunk.sections.len() as i32;
                        let changed: Vec<i32> = (0..n)
                            .filter(|&s| {
                                !Arc::ptr_eq(&old.sections[s as usize], &chunk.sections[s as usize])
                            })
                            .collect();
                        (0..n)
                            .filter(|s| changed.iter().any(|c| (c - s).abs() <= 1))
                            .collect()
                    }
                    _ => (0..chunk.sections.len() as i32).collect(),
                };
                if sections.is_empty()
                    || region
                        .meshed
                        .get(&(x, z))
                        .is_some_and(|(_, at)| now.duration_since(*at) < REMESH_AFTER)
                {
                    continue;
                }
                if self.streaming.mesh_foreign(
                    i as u8,
                    &chunks,
                    x,
                    z,
                    &sections,
                    dim == Dim::Nether,
                ) {
                    region.meshed.insert((x, z), (chunk.clone(), now));
                }
            }
        }
        self.portal_meshed.extend(self.streaming.take_foreign());
        if self.portal_meshed.is_empty() {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let sections = self.world.layout().sections() as i32;
        let mut spent = Vec::new();
        let started = Instant::now();
        while started.elapsed() < FOREIGN_BUDGET {
            let Some((tag, (x, sy, z), mesh)) = self.portal_meshed.pop_front() else {
                break;
            };
            let dim = LEVELS[tag as usize];
            let Some(bottom) = views.iter().find(|v| v.request.to == dim).map(|v| v.bottom_y) else {
                spent.push(((x, sy, z), mesh));
                continue;
            };
            let region = &mut self.portal_meshes.regions[tag as usize];
            // Section indices count from the bottom of the window's level, not the other's.
            let y = sy + (bottom - self.world.bottom_y()).div_euclid(16);
            if (0..sections).contains(&y) {
                renderer.upload((x + region_x(dim), y, z), &mesh);
                region.uploaded += 1;
            }
            if let Some((_, old)) = region.cache.insert((x, sy, z), (bottom, mesh)) {
                spent.push(((x, sy, z), old));
            }
        }
        self.streaming.free(spent);
        if self.streaming.foreign_pending() == 0 {
            for (i, region) in self.portal_meshes.regions.iter_mut().enumerate() {
                if !region.logged && !region.meshed.is_empty() {
                    region.logged = true;
                    log::info!(
                        "Oxyra: portal view meshed {} sections of {} columns of {:?}",
                        region.uploaded,
                        region.meshed.len(),
                        LEVELS[i]
                    );
                }
            }
        }
    }

    /// The dimension behind the nearest portal the window sees through, as packs number it.
    pub(super) fn portal_view_dimension(&self) -> Option<i32> {
        self.portal_entries
            .iter()
            .find(|e| e.parent.is_none())
            .map(|e| match e.request.to {
                Dim::Overworld => 0,
                Dim::Nether => -1,
                Dim::End => 1,
            })
    }

    /// Whether the crosshair leaves `state` unmarked: a portal the window sees through is no
    /// block to the eye (it can still be broken).
    pub(super) fn portal_outline_hidden(&self, state: BlockStateId) -> bool {
        let mut keep = false;
        self.native_portal_outline(&mut keep);
        !keep && self.portal_view_on.0 && self.res.behaviours().portal_axis(state).is_some()
    }

    /// A Nether portal's way out of `level` for what enters it at the point of its blocks nearest
    /// `at` (its middle without one): the level it leads to and where its exit is looked for
    /// (`NetherPortalBlock.getPortalDestination`), which picks the exit the crossing would.
    fn portal_way(
        &self,
        level: Dim,
        area: &FoundRectangle,
        x_axis: bool,
        at: Option<Vec3>,
    ) -> (Dim, BlockPos) {
        let [low, high] = portal_box(area, x_axis, 0.0);
        let at = at.map_or((low + high) / 2.0, |p| {
            glam::DVec3::new(p.x, p.y, p.z).clamp(low, high)
        });
        // DimensionType.getTeleportationScale: 8 out of the Nether, 1/8 into it.
        let (to, scale) = if level == Dim::Nether {
            (Dim::Overworld, 8.0)
        } else {
            (Dim::Nether, 0.125)
        };
        (to, self.border_bounds().clamp(at.x * scale, at.y, at.z * scale))
    }

    /// The window's side, each tick: the levels behind the portals near the player and behind
    /// those their views show, asked of the integrated server, or nothing asked.
    pub(super) fn request_portal_view(&mut self) {
        if self.integrated.is_none() {
            return;
        }
        let (mut view, mut crossing) = (false, false);
        self.native_portal_view(&mut view, &mut crossing);
        // For testing the engine without the mod: both on.
        if std::env::var_os("HORIZON_PORTAL_VIEW").is_some() {
            (view, crossing) = (true, true);
        }
        self.portal_view_on = (view, view && crossing);
        if !view {
            self.portal_entries.clear();
            self.portal_far.clear();
            self.hide_portal_faces(false);
            if let Some(server) = self.integrated.as_ref() {
                server.request_portal_views(Vec::new());
            }
            self.portal_view_seen.clear();
            return;
        }
        self.hide_portal_faces(true);
        let b = self.res.behaviours();
        let registry = self.pack.assets.registry;
        let dim = self.dim;
        let mut far = Vec::new();
        let mut entries: Vec<PortalEntry> = Vec::new();
        if let Some(p) = self.player.as_ref()
            && dim != Dim::End
        {
            let eye = [p.position.x, p.position.y + 1.6, p.position.z];
            let chunks = &self.world.chunks;
            let found = portal_rectangles(
                |c| chunks.get(&c).map(|c| &**c),
                eye,
                PLAIN_RANGE,
                |v| {
                    b.portal_axis(v).map(Some).or_else(|| {
                        (dim == Dim::Overworld
                            && registry
                                .state(v)
                                .is_some_and(|st| st.name == "minecraft:end_portal"))
                        .then_some(None)
                    })
                },
            );
            let near = |a: &FoundRectangle| {
                let m = a.min_corner;
                let d = [f64::from(m.x), f64::from(m.y), f64::from(m.z)];
                d.iter().zip(eye).all(|(c, e)| (c - e).abs() <= f64::from(RANGE))
            };
            for (area, x_axis) in found {
                if !near(&area) || entries.len() == MAX_PORTAL_VIEWS {
                    if let Some(x_axis) = x_axis {
                        far.push((area, x_axis));
                    }
                    continue;
                }
                let from = (dim, area.min_corner);
                let request = if let Some(x_axis) = x_axis {
                    let (to, approximate) =
                        self.portal_way(dim, &area, x_axis, self.player.as_ref().map(|p| p.position));
                    PortalRequest {
                        from,
                        to,
                        approximate,
                        entry: None,
                        crossing,
                    }
                } else {
                    // `EndPortalBlock.getPortalDestination`: always the spawn platform.
                    PortalRequest {
                        from,
                        to: Dim::End,
                        approximate: BlockPos::new(END_SPAWN[0], END_SPAWN[1], END_SPAWN[2]),
                        entry: Some(area),
                        crossing,
                    }
                };
                entries.push(PortalEntry {
                    area,
                    x_axis,
                    request,
                    parent: None,
                    depth: 1,
                });
            }
        }
        // The Nether portals in the views, breadth first: nearer openings keep the budget.
        let views = self
            .integrated
            .as_ref()
            .map_or_else(Vec::new, |s| s.portal_views());
        let mut next = 0;
        while next < entries.len() && entries.len() < MAX_PORTAL_VIEWS {
            let parent = entries[next];
            next += 1;
            let level = parent.request.to;
            if parent.depth >= DEPTH || level == Dim::End {
                continue;
            }
            let Some(view) = views.iter().find(|v| v.request.from == parent.request.from) else {
                continue;
            };
            let Some(exit) = view.exit else {
                continue;
            };
            let chunks: FxHashMap<_, _> = view.chunks.iter().map(|(k, c)| (*k, &**c)).collect();
            let m = exit.min_corner;
            let around = [f64::from(m.x), f64::from(m.y), f64::from(m.z)];
            let found = portal_rectangles(
                |c| chunks.get(&c).copied(),
                around,
                RANGE,
                |v| b.portal_axis(v).map(Some),
            );
            for (area, x_axis) in found {
                if area == exit || entries.len() == MAX_PORTAL_VIEWS {
                    continue;
                }
                let (to, approximate) =
                    self.portal_way(level, &area, x_axis.unwrap_or(true), None);
                entries.push(PortalEntry {
                    area,
                    x_axis,
                    request: PortalRequest {
                        from: (level, area.min_corner),
                        to,
                        approximate,
                        entry: None,
                        crossing,
                    },
                    parent: Some(next - 1),
                    depth: parent.depth + 1,
                });
            }
        }
        let mut requests: Vec<PortalRequest> = Vec::new();
        for e in &entries {
            if !requests.iter().any(|r| r.from == e.request.from) {
                requests.push(e.request);
            }
        }
        self.portal_entries = entries;
        self.portal_far = far;
        let Some(server) = self.integrated.as_ref() else {
            return;
        };
        server.request_portal_views(requests);
        // The log tells when what is seen through portals changes.
        let mut seen: Vec<_> = views
            .iter()
            .map(|v| {
                let exit = v
                    .exit
                    .map(|e| [e.min_corner.x, e.min_corner.y, e.min_corner.z]);
                let from = v.request.from.1;
                (v.request.to, [from.x, from.y, from.z], exit, v.chunks.len())
            })
            .collect();
        seen.sort_by_key(|(to, from, ..)| (region_of(*to), *from));
        if seen != self.portal_view_seen {
            for (to, from, exit, chunks) in &seen {
                log::info!(
                    "Oxyra: portal view from {from:?} into {to:?}, exit {exit:?}, {chunks} chunks"
                );
            }
            self.portal_view_seen = seen;
        }
    }

    /// Whether the mesher leaves out portal blocks' faces: the views and the plain quads draw
    /// them, and a shader pack's light and shadows pass through the frame as through a doorway.
    fn hide_portal_faces(&mut self, hide: bool) {
        if hide == !self.streaming.hidden_blocks.is_empty() {
            return;
        }
        let b = self.res.behaviours();
        let hidden: Vec<u32> = if hide {
            (0..self.pack.assets.registry.len() as u32)
                .filter(|&id| b.portal_axis(BlockStateId(id)).is_some())
                .collect()
        } else {
            Vec::new()
        };
        self.streaming.hidden_blocks = hidden.into();
        self.all_sections_changed();
    }

    /// The portals that show no view this frame (no exit yet, too far), in their sprite.
    pub(super) fn plain_portal_geometry(
        &self,
        camera: glam::DVec3,
        plain: &[(FoundRectangle, bool)],
        out: &mut Vec<Vertex>,
    ) {
        let Some(sprite) = self.gui.atlas.sprites.get("block/nether_portal") else {
            return;
        };
        for (area, x_axis) in plain.iter().chain(&self.portal_far) {
            let m = area.min_corner;
            let (w, h) = (area.axis1_size as f32, area.axis2_size as f32);
            let at = (glam::DVec3::new(f64::from(m.x), f64::from(m.y), f64::from(m.z)) - camera)
                .as_vec3();
            let along = if *x_axis { glam::Vec3::X } else { glam::Vec3::Z };
            let across = if *x_axis { glam::Vec3::Z } else { glam::Vec3::X };
            let light = self.world.light(m.x, m.y, m.z);
            let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
            let base = at + across * 0.5;
            // One quad per block, so the sprite repeats; both sides.
            for a in 0..w as i32 {
                for y in 0..h as i32 {
                    let corner = base + along * a as f32 + glam::Vec3::Y * y as f32;
                    let quad = [
                        (corner, [sprite.u0, sprite.v1]),
                        (corner + along, [sprite.u1, sprite.v1]),
                        (corner + along + glam::Vec3::Y, [sprite.u1, sprite.v0]),
                        (corner + glam::Vec3::Y, [sprite.u0, sprite.v0]),
                    ];
                    for order in [[0, 1, 2, 3], [3, 2, 1, 0]] {
                        for i in order {
                            out.push(Vertex {
                                pos: quad[i].0.to_array(),
                                uv: quad[i].1,
                                layer: 0,
                                color: u32::MAX,
                                light: packed,
                                normal: [0; 4],
                                glint_uv: [0.0; 2],
                            });
                        }
                    }
                }
            }
        }
    }

    /// What the window sees through portals from `camera` this frame, nearest first, each after
    /// the view it is seen in, and the portals in the window that wait for one (their exit to be
    /// found, their level meshed).
    pub(super) fn portal_frames(
        &self,
        camera: glam::DVec3,
    ) -> (
        Vec<horizon_client::gfx::PortalView>,
        Vec<(FoundRectangle, bool)>,
    ) {
        use glam::DVec3;
        let views = self
            .integrated
            .as_ref()
            .map_or_else(Vec::new, |s| s.portal_views());
        self.portal_hop.set(None);
        let mut frames: Vec<horizon_client::gfx::PortalView> = Vec::new();
        let mut frame_of = vec![None; self.portal_entries.len()];
        let mut plain = Vec::new();
        for (i, entry) in self.portal_entries.iter().enumerate() {
            // The camera it is seen from, in the coordinates of the level it is in.
            let (from, yaw, parent, east) = match entry.parent {
                None => (camera, 0.0, None, 0.0),
                Some(q) => {
                    let Some(f) = frame_of[q] else {
                        continue;
                    };
                    let seen_in: &horizon_client::gfx::PortalView = &frames[f];
                    let east =
                        f64::from(region_x(self.portal_entries[q].request.to)) * 16.0;
                    (seen_in.camera - DVec3::X * east, seen_in.yaw, Some(f), east)
                }
            };
            let Some(view) = views.iter().find(|v| v.request.from == entry.request.from) else {
                if let (None, Some(x_axis)) = (entry.parent, entry.x_axis) {
                    plain.push((entry.area, x_axis));
                }
                continue;
            };
            // The stored light in the entry portal, in its own level.
            let m = entry.area.min_corner;
            let (lx, ly, lz) = match entry.x_axis {
                Some(_) => (m.x, m.y + entry.area.axis2_size / 2, m.z),
                None => (m.x, m.y + 1, m.z),
            };
            let entry_light = match entry.parent {
                None => self.world.light(lx, ly, lz),
                Some(q) => views
                    .iter()
                    .find(|v| v.request.from == self.portal_entries[q].request.from)
                    .and_then(|v| v.chunks.iter().find(|(k, _)| *k == (lx >> 4, lz >> 4)))
                    .map_or(0xF0, |(_, c)| c.light(lx & 15, ly, lz & 15)),
            };
            let Some(frame) = self.portal_frame(entry, view, from, yaw, parent, east, entry_light)
            else {
                if let (None, Some(x_axis)) = (entry.parent, entry.x_axis) {
                    plain.push((entry.area, x_axis));
                }
                continue;
            };
            // Only what lies past the parent's exit can be seen in its view.
            if let Some(f) = parent {
                let [a, b, c, d] = frames[f].clip.map(f64::from);
                let m = entry.area.min_corner;
                let relative = DVec3::new(f64::from(m.x), f64::from(m.y), f64::from(m.z))
                    + DVec3::new(0.5, 0.5, 0.5)
                    - from;
                if DVec3::new(a, b, c).dot(relative) + d <= 0.0 {
                    continue;
                }
            }
            frame_of[i] = Some(frames.len());
            frames.push(frame);
        }
        (frames, plain)
    }

    /// Whether the camera is on the low side of the portal with corner `corner`, `depth` blocks
    /// into its thickness: inside it, the side it came in from, until it crosses (the server's
    /// answer comes a few ticks after its middle plane).
    fn portal_side(&self, corner: BlockPos, depth: f64) -> bool {
        let low = depth < 0.5;
        if !(0.0..=1.0).contains(&depth) {
            if self.portal_side.get().is_some_and(|(c, _)| c == corner) {
                self.portal_side.set(None);
            }
            return low;
        }
        match self.portal_side.get() {
            Some((c, side)) if c == corner => side,
            _ => {
                self.portal_side.set(Some((corner, low)));
                low
            }
        }
    }

    /// The new level's first frame of a crossing: where the camera's portal takes the player,
    /// as its view showed it (`remote_teleport`).
    pub(super) fn note_portal_jump(&mut self) {
        self.portal_jump = self
            .portal_hop
            .get()
            .filter(|h| h.to_level == self.dim)
            .zip(self.player.as_ref())
            .map(|(h, p)| {
                let turned = glam::DMat3::from_rotation_y(h.turn);
                let at = glam::DVec3::new(p.position.x, p.position.y, p.position.z);
                let v = glam::DVec3::new(p.velocity.x, p.velocity.y, p.velocity.z);
                let (at, v) = (h.to + turned * (at - h.from), turned * v);
                PortalJump {
                    position: Vec3::new(at.x, at.y, at.z),
                    velocity: Vec3::new(v.x, v.y, v.z),
                    at: Instant::now(),
                }
            });
    }

    /// Where a teleport the server sent to `position` lands the player instead, and its velocity:
    /// the landing of the crossing it answers, when it is that one.
    pub(super) fn portal_landing(&mut self, position: Vec3) -> Option<(Vec3, Vec3)> {
        let jump = self.portal_jump.as_ref()?;
        let near = |a: f64, b: f64| (a - b).abs() < 3.0;
        if jump.at.elapsed() > Duration::from_secs(3) {
            self.portal_jump = None;
            return None;
        }
        if !(near(jump.position.x, position.x)
            && near(jump.position.y, position.y)
            && near(jump.position.z, position.z))
        {
            return None;
        }
        log::info!(
            "Oxyra: landed {} ms after the crossing's respawn",
            jump.at.elapsed().as_millis()
        );
        self.portal_jump.take().map(|j| (j.position, j.velocity))
    }

    /// What `camera` sees through the portal `entry`, in the level `view` holds: the camera
    /// carried from the entry to the exit (turned a quarter when their axes differ, as the
    /// crossing turns the player), the exit's plane, and the entry's opening `east` blocks east,
    /// as the renderer holds the level it is in.
    fn portal_frame(
        &self,
        entry: &PortalEntry,
        view: &PortalSnapshot,
        camera: glam::DVec3,
        seen_yaw: f32,
        parent: Option<usize>,
        east: f64,
        entry_light: u8,
    ) -> Option<horizon_client::gfx::PortalView> {
        use glam::DVec3;
        let area = &entry.area;
        if self.portal_meshes.regions[region_of(view.request.to)]
            .meshed
            .is_empty()
        {
            return None;
        }
        let m = area.min_corner;
        let base = DVec3::new(f64::from(m.x), f64::from(m.y), f64::from(m.z));
        let (w, h) = (f64::from(area.axis1_size), f64::from(area.axis2_size));
        // Each portal's plane: the points the entry's and the exit's match at, the turn between
        // them, their axes across, and the entry's opening on its plane.
        // A Nether portal: the way the camera goes through it, from the side it came from.
        let mut heading = None;
        let (from, to, turn, exit_axis, opening) = match entry.x_axis {
            // A Nether portal: its blocks' middle plane, where their faces give way, to the
            // exit's, turned a quarter when their axes differ as the crossing turns the player.
            Some(x_axis) => {
                let exit = view.exit.filter(|_| view.request.to != Dim::End)?;
                let middle = |a: &FoundRectangle, x_axis: bool| {
                    let m = a.min_corner;
                    let (w, h) = (f64::from(a.axis1_size), f64::from(a.axis2_size));
                    DVec3::new(
                        f64::from(m.x) + if x_axis { w / 2.0 } else { 0.5 },
                        f64::from(m.y) + h / 2.0,
                        f64::from(m.z) + if x_axis { 0.5 } else { w / 2.0 },
                    )
                };
                let (along, across) = if x_axis {
                    (DVec3::X, DVec3::Z)
                } else {
                    (DVec3::Z, DVec3::X)
                };
                // The far face of the frame: all of the frame is this level's, whatever the side.
                // A hair inside it: a block against the frame has its face on that plane.
                let depth = across.dot(camera - base);
                let low_side = if parent.is_none() {
                    self.portal_side(m, depth)
                } else {
                    depth < 0.5
                };
                heading = Some(if low_side { across } else { -across });
                let far = if low_side {
                    1.0 - PORTAL_OPENING_INSET
                } else {
                    PORTAL_OPENING_INSET
                };
                let corner = |a: f64, y: f64| base + far * across + a * along + y * DVec3::Y;
                (
                    middle(area, x_axis),
                    middle(&exit, view.exit_x_axis),
                    // `NetherPortalBlock.createDimensionTransition` adds 90 to the yaw between
                    // axes: facing +z turns to -x.
                    if x_axis == view.exit_x_axis {
                        0.0
                    } else {
                        -std::f64::consts::FRAC_PI_2
                    },
                    if view.exit_x_axis { DVec3::Z } else { DVec3::X },
                    [
                        corner(0.0, 0.0),
                        corner(w, 0.0),
                        corner(w, h),
                        corner(0.0, h),
                    ],
                )
            }
            // An End portal, seen from above: its top (12/16), to where the crossing lands
            // (`EndPortalBlock.getPortalDestination`). The opening lies a hair over the top
            // face, which it covers.
            None => {
                let top = base + DVec3::new(w / 2.0, 0.75, h / 2.0);
                // From under it too: the opening a hair below the block, looking up from the
                // carried camera.
                let above = camera.y > top.y;
                if view.request.to != Dim::End || !(above || camera.y < base.y) {
                    return None;
                }
                let plane = if above { 0.75 + 1.0 / 256.0 } else { -1.0 / 256.0 };
                let corner = |a: f64, b: f64| base + DVec3::new(a, plane, b);
                (
                    top,
                    DVec3::new(
                        f64::from(END_SPAWN[0]) + 0.5,
                        f64::from(END_SPAWN[1] - 1),
                        f64::from(END_SPAWN[2]) + 0.5,
                    ),
                    0.0,
                    DVec3::Y,
                    [
                        corner(0.0, 0.0),
                        corner(w, 0.0),
                        corner(w, h),
                        corner(0.0, h),
                    ],
                )
            }
        };
        // The level behind's sky at the exit (`EnvironmentAttributeProbe` at its biome there).
        let environment = match view.request.to {
            Dim::Overworld => self.res.environment(),
            Dim::Nether => self.res.nether_environment(),
            Dim::End => self.res.end_environment(),
        };
        let biomes = world_biomes!(self);
        let at = to.floor().as_ivec3();
        let biome = view
            .chunks
            .iter()
            .find(|(c, _)| *c == (at.x >> 4, at.z >> 4))
            .map(|(_, c)| biomes.get(c.biome(at.x, at.y, at.z)));
        let colour = |pick: Option<u32>, name: &str| {
            pick.map_or(environment.base_rgb(name), horizon_client::biome::rgb)
        };
        let sky = environment.sample(
            self.world.day_time,
            Some(colour(
                biome.and_then(|b| b.sky),
                "minecraft:visual/sky_color",
            )),
            Some(colour(
                biome.and_then(|b| b.fog),
                "minecraft:visual/fog_color",
            )),
        );
        let turned = glam::DMat3::from_rotation_y(turn);
        let offset = turned * (camera - from);
        if parent.is_none()
            && entry.x_axis.is_some()
            && let Some(exit) = view.exit
            && (exit.axis1_size, exit.axis2_size) == (area.axis1_size, area.axis2_size)
        {
            let inside = |v: f64, size: f64| (-0.5..=size + 0.5).contains(&v);
            let (along, across) = if entry.x_axis == Some(true) {
                (camera.x - base.x, camera.z - base.z)
            } else {
                (camera.z - base.z, camera.x - base.x)
            };
            if inside(along, w) && (-0.5..=1.5).contains(&across) {
                self.portal_hop.set(Some(PortalHop {
                    to_level: view.request.to,
                    from,
                    to,
                    turn,
                }));
            }
        }
        let beyond = f64::from(region_x(view.request.to)) * 16.0;
        let carried = to + offset + DVec3::new(beyond, 0.0, 0.0);
        // The exit's plane, its normal away from the carried camera: what lies before it is cut.
        let normal = match heading {
            Some(heading) if exit_axis.dot(turned * heading) > 0.0 => exit_axis,
            Some(_) => -exit_axis,
            None if exit_axis.dot(offset) > 0.0 => -exit_axis,
            None => exit_axis,
        };
        Some(horizon_client::gfx::PortalView {
            parent,
            camera: carried,
            yaw: seen_yaw - turn as f32,
            clip: [
                normal.x as f32,
                normal.y as f32,
                normal.z as f32,
                // The End's platform, a block thick under where the crossing lands, is left out: a
                // look down the portal would show only its obsidian, not the End.
                (normal.dot(offset)
                    - if entry.x_axis.is_none() && offset.y > 0.0 {
                        1.01
                    } else {
                        0.0
                    }) as f32,
            ],
            opening: opening.map(|c| c + DVec3::X * east),
            dimension: match view.request.to {
                Dim::Overworld => 0,
                Dim::Nether => -1,
                Dim::End => 1,
            },
            entry_light,
            fog: sky.fog_color,
            clear: if view.request.to == Dim::End {
                [0.0; 3]
            } else {
                sky.fog_color
            },
            sky,
            environment_fog: (view.request.to == Dim::Nether)
                .then_some((sky.fog_start, sky.fog_end)),
            entities: Arc::default(),
            entities_at: carried,
        })
    }

    /// The server's side, as the window's player crosses an End portal it sees through: where
    /// the view showed it: carried across from the portal's middle to the spawn platform's
    /// (`EndPortalBlock.getPortalDestination` puts it on the platform's middle, facing west).
    pub(super) fn portal_view_landing(&self, feet: Vec3) -> Option<Vec3> {
        let request = self
            .portal_view_requests()
            .into_iter()
            .find(|r| r.to == Dim::End && r.crossing)?;
        let area = request.entry?;
        let m = area.min_corner;
        Some(Vec3::new(
            f64::from(END_SPAWN[0]) + 0.5 + feet.x
                - (f64::from(m.x) + f64::from(area.axis1_size) / 2.0),
            // On the platform, as vanilla: the feet are a hair below the portal's top as it
            // takes them, and a landing in the air falls before the level has loaded.
            f64::from(END_SPAWN[1] - 1),
            f64::from(END_SPAWN[2]) + 0.5 + feet.z
                - (f64::from(m.z) + f64::from(area.axis2_size) / 2.0),
        ))
    }

    /// The server's side, in the level `request` leads to: its exit portal as
    /// `NetherPortalBlock.getExitPortal` finds one (the closest within reach), never building
    /// one; searched again only when the request changes or every `SEARCH_EVERY` ticks.
    pub(super) fn portal_view_exit(
        &mut self,
        request: PortalRequest,
    ) -> Option<(FoundRectangle, bool)> {
        let now = self.world.game_time.max(0) as u64;
        // Searches no request has asked again for a while go.
        self.portal_exit_search
            .retain(|s| now.saturating_sub(s.at) < 2 * SEARCH_EVERY);
        if let Some(search) = self
            .portal_exit_search
            .iter()
            .find(|s| s.request == request && now.saturating_sub(s.at) < SEARCH_EVERY)
        {
            return search.exit;
        }
        let to_nether = request.to == Dim::Nether;
        let radius = if to_nether { 16 } else { 128 };
        let portals = self.portal_blocks(request.approximate, radius);
        let (min_section, max_section) = if to_nether { (0, 15) } else { (-4, 19) };
        let border = self.border_bounds();
        let exit = closest_portal(
            &portals,
            request.approximate,
            to_nether,
            &border,
            min_section,
            max_section,
        )
        .map(|found| {
            let state = self.world.block(found.x, found.y, found.z);
            let x_axis = self.res.behaviours().portal_axis(state).unwrap_or(true);
            let area = largest_rectangle_around(
                found,
                if x_axis { Axis::X } else { Axis::Z },
                21,
                Axis::Y,
                21,
                |p| self.world.block(p.x, p.y, p.z) == state,
            );
            (area, x_axis)
        });
        self.portal_exit_search.retain(|s| s.request != request);
        self.portal_exit_search.push(ExitSearch {
            request,
            exit,
            at: now,
        });
        exit
    }

    /// The server's side: the entities of the level in place, and the players in it but the
    /// window's own.
    pub(super) fn portal_view_entities(&self, dim: Dim) -> Arc<PortalEntities> {
        let mobs = self.mobs.clone();
        let mut players = Vec::new();
        if let Some(host) = &self.lan {
            for g in host.hub.guests.iter().filter(|g| {
                g.placed
                    && g.dimension == dim.id()
                    && host.owner.as_ref().is_none_or(|o| o.uuid != g.uuid)
            }) {
                let at = |p: [f64; 3]| Vec3::new(p[0], p[1], p[2]);
                players.push(PortalPlayer {
                    id: g.entity_id as u32,
                    uuid: g.uuid,
                    position: at(g.position),
                    previous: at(g.previous_position),
                    yaw: g.yaw,
                    pitch: g.pitch,
                    head_yaw: g.head_yaw,
                });
            }
        }
        Arc::new(PortalEntities {
            mobs,
            dropped: self.dropped.clone(),
            armor_stands: self.armor_stands.clone(),
            boats: self.boats.clone(),
            carts: self.carts.clone(),
            players,
        })
    }

    /// The window's side, each frame: the entities of the levels behind portals, as each view's
    /// camera sees them. The window's own lists are set aside meanwhile.
    pub(super) fn portal_entity_vertices(
        &mut self,
        portals: &mut [horizon_client::gfx::PortalView],
        alpha: f64,
    ) {
        if portals.is_empty() {
            return;
        }
        let views = self
            .integrated
            .as_ref()
            .map_or_else(Vec::new, |s| s.portal_views());
        for (i, &dim) in LEVELS.iter().enumerate() {
            let Some(view) = views.iter().find(|v| v.request.to == dim) else {
                continue;
            };
            let region = &mut self.portal_meshes.regions[i];
            if region
                .entities_from
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &view.entities))
            {
                region.entities = (*view.entities).clone();
                region.entities_from = Some(view.entities.clone());
                let players = region.entities.players.clone();
                for player in players {
                    if let Some(mob) = self.foreign_player_mob(&player) {
                        self.portal_meshes.regions[i].entities.mobs.push(mob);
                    }
                }
                let region = &mut self.portal_meshes.regions[i];
                for v in views.iter().filter(|v| v.request.to == dim) {
                    for (k, chunk) in &v.chunks {
                        region.light.insert(*k, chunk.clone());
                    }
                }
            }
        }
        let spans = self.entity_spans.get_mut().len();
        let motion = self.entity_motion.get_mut().len();
        let rendered = self.rendered_mobs.get();
        let culler = self.entity_culler.take();
        let async_culler = self.async_entity_culler.take();
        self.portal_entity_pass.set(true);
        for p in portals.iter_mut() {
            let i = ((p.camera.x / (f64::from(PORTAL_X) * 16.0)).round() as usize)
                .saturating_sub(1)
                .min(LEVELS.len() - 1);
            let beyond = f64::from(region_x(LEVELS[i])) * 16.0;
            let camera = p.camera - glam::DVec3::X * beyond;
            self.swap_portal_entities(i);
            let mut out = Vec::new();
            self.mob_geometry(camera, alpha, &mut out);
            self.dropped_item_geometry(camera, alpha, &mut out);
            self.armor_stand_geometry(camera, alpha, &mut out);
            self.boat_geometry(camera, alpha, &mut out);
            self.cart_geometry(camera, alpha, &mut out);
            self.swap_portal_entities(i);
            p.entities = Arc::new(out);
        }
        self.portal_entity_pass.set(false);
        self.entity_culler = culler;
        self.async_entity_culler = async_culler;
        self.rendered_mobs.set(rendered);
        self.entity_spans.get_mut().truncate(spans);
        self.entity_motion.get_mut().truncate(motion);
    }

    fn swap_portal_entities(&mut self, i: usize) {
        let region = &mut self.portal_meshes.regions[i];
        std::mem::swap(&mut self.mobs, &mut region.entities.mobs);
        std::mem::swap(&mut self.dropped, &mut region.entities.dropped);
        std::mem::swap(&mut self.armor_stands, &mut region.entities.armor_stands);
        std::mem::swap(&mut self.boats, &mut region.entities.boats);
        std::mem::swap(&mut self.carts, &mut region.entities.carts);
        std::mem::swap(&mut self.world.chunks, &mut region.light);
    }

    /// The server's side: the snapshot of the level in place for `request`, its exit's chunks
    /// kept running with a portal ticket.
    pub(super) fn portal_view_snapshot(
        &mut self,
        request: PortalRequest,
        entities: Arc<PortalEntities>,
    ) -> PortalSnapshot {
        let exit = if request.to == Dim::End {
            None
        } else {
            self.portal_view_exit(request)
        };
        let centre = exit.map_or(request.approximate, |(area, _)| area.min_corner);
        let (cx, cz) = (centre.x >> 4, centre.z >> 4);
        // `TicketType.PORTAL`: the exit's area stays loaded and its level running.
        // One portal ticket holds 7x7 chunks: a grid of them holds the whole view.
        for tz in (-VIEW_CHUNKS..=VIEW_CHUNKS).step_by(7) {
            for tx in (-VIEW_CHUNKS..=VIEW_CHUNKS).step_by(7) {
                self.place_portal_ticket((cx + tx + 3, cz + tz + 3));
            }
        }
        let mut chunks = Vec::new();
        for z in cz - VIEW_CHUNKS..=cz + VIEW_CHUNKS {
            for x in cx - VIEW_CHUNKS..=cx + VIEW_CHUNKS {
                if let Some(chunk) = self.world.chunks.get(&(x, z)) {
                    chunks.push(((x, z), chunk.clone()));
                }
            }
        }
        PortalSnapshot {
            request,
            exit: exit.map(|e| e.0),
            exit_x_axis: exit.is_none_or(|e| e.1),
            chunks,
            bottom_y: self.world.bottom_y(),
            entities,
        }
    }
}
