//! Options > Mods as Mod Menu lays it out (`ModsScreen`), when a mod asks for it
//! (`native_mod_menu`): the list on the left with its search, the selected mod's details on the
//! right. A mod is turned on or off here too, as Oxyra's page does. Built from vanilla's widgets.
use crate::{
    hud::{HudAtlas, HudBatch, rgba},
    lang,
    mod_list::{Installed, ModList},
    text::Text,
    ui,
};

/// `ModsScreen.RIGHT_PANE_Y`.
const RIGHT_PANE_Y: f32 = 48.0;
/// `ModsScreen`'s list top with its count line, and its row height.
const LIST_Y: f32 = 67.0;
const ROW: f32 = 36.0;
const ICON: f32 = 32.0;
/// Mods other mods build on, under the Libraries toggle.
const LIBRARIES: &[&str] = &["oxyra_api"];

fn tr(key: &str, english: &str) -> String {
    lang::get_or(key, english)
}

/// Sort by name, either way (`ModMenuConfig.SORTING`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Sort {
    #[default]
    Ascending,
    Descending,
}

/// What a press on this page did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Nothing,
    Pressed,
    Toggle(usize),
    Settings(usize),
    Folder,
    Done,
}

pub struct ModMenuUi {
    pub search: ui::EditBox,
    selected: Option<usize>,
    /// The first row shown: the list scrolls by rows.
    first: usize,
    filters: bool,
    sort: Sort,
    libraries: bool,
}

impl Default for ModMenuUi {
    fn default() -> Self {
        Self {
            search: ui::EditBox::new(0.0, 22.0, 200.0, 20.0),
            selected: None,
            first: 0,
            filters: false,
            sort: Sort::default(),
            libraries: true,
        }
    }
}

/// `title` is `Name 1.0`: the name and the version.
fn name_and_version(title: &str) -> (&str, &str) {
    title.rsplit_once(' ').unwrap_or((title, ""))
}

/// The mods shown, in order: those the search finds, libraries as the toggle says.
fn shown(mods: &[Installed], query: &str, sort: Sort, libraries: bool) -> Vec<usize> {
    let query = query.to_lowercase();
    let mut rows: Vec<usize> = (0..mods.len())
        .filter(|&i| libraries || !LIBRARIES.contains(&mods[i].id.as_str()))
        .filter(|&i| {
            mods[i].title.to_lowercase().contains(&query)
                || mods[i].description.to_lowercase().contains(&query)
                || mods[i].id.contains(&query)
        })
        .collect();
    rows.sort_by_key(|&i| mods[i].title.to_lowercase());
    if sort == Sort::Descending {
        rows.reverse();
    }
    rows
}

impl ModMenuUi {
    pub fn reset(&mut self) {
        self.search.set_value("");
        self.search.focused = false;
        self.first = 0;
    }

    fn pane(w: f32) -> f32 {
        (w / 2.0).floor() - 8.0
    }

    /// `ModsScreen`'s search row: the box centred in the left pane, the filters button after it.
    fn search_rect(w: f32) -> [f32; 3] {
        let pane = Self::pane(w);
        let width = (pane - 32.0 - 20.0).clamp(40.0, 200.0);
        let x = (pane / 2.0 - width / 2.0 - 10.0).floor();
        [x, 22.0, width]
    }

    fn rows_fit(h: f32) -> usize {
        ((h - LIST_Y - 36.0) / ROW).floor().max(1.0) as usize
    }

    fn bottom(w: f32, h: f32) -> [[f32; 4]; 2] {
        let pane = Self::pane(w);
        let width = (pane - 8.0).min(150.0);
        [
            [(pane / 2.0 - width / 2.0).floor(), h - 28.0, width, 20.0],
            [
                (w - pane + pane / 2.0 - width / 2.0).floor(),
                h - 28.0,
                width,
                20.0,
            ],
        ]
    }

    /// The right pane's buttons: on or off, then the settings when the mod has any.
    fn detail_buttons(w: f32) -> [[f32; 4]; 2] {
        let (pane, x) = (Self::pane(w), w - Self::pane(w));
        let width = (pane / 2.0 - 2.0).floor();
        [
            [x, RIGHT_PANE_Y + 36.0, width, 20.0],
            [x + width + 4.0, RIGHT_PANE_Y + 36.0, width, 20.0],
        ]
    }

    fn sort_label(&self) -> Text {
        Text::literal(match self.sort {
            Sort::Ascending => tr("horizon.mod_menu.sort_asc", "Sort: A-Z"),
            Sort::Descending => tr("horizon.mod_menu.sort_desc", "Sort: Z-A"),
        })
    }

    fn libraries_label(&self) -> Text {
        Text::literal(if self.libraries {
            tr("horizon.mod_menu.libraries_shown", "Libraries: Shown")
        } else {
            tr("horizon.mod_menu.libraries_hidden", "Libraries: Hidden")
        })
    }

    /// The filters' row (`filtersX`, y 45): the sort and libraries buttons.
    fn filter_buttons(&self, atlas: &HudAtlas, w: f32) -> [[f32; 4]; 2] {
        let width = |t: Text| HudBatch::text_width(atlas, &t.plain()).ceil() + 20.0;
        let (a, b) = (width(self.sort_label()), width(self.libraries_label()));
        let x = (Self::pane(w) - a - b - 2.0 - 4.0).max(0.0);
        [[x, 45.0, a, 20.0], [x + a + 2.0, 45.0, b, 20.0]]
    }

    pub fn draw(&mut self, mods: &ModList, batch: &mut HudBatch, atlas: &HudAtlas, cursor: (f32, f32), in_world: bool) {
        let (w, h) = (batch.width.ceil(), batch.height.ceil());
        ui::menu_background(batch, atlas, 0.0, 0.0, w, h, in_world);
        let installed = &mods.installed;
        let pane = Self::pane(w);
        let grey = rgba(128, 128, 128, 255);
        let light = rgba(170, 170, 170, 255);
        ui::centered_text(
            batch,
            atlas,
            &Text::literal(tr("horizon.pages.mods", "Mods")),
            0.0,
            8.0,
            pane,
            u32::MAX,
        );
        // Search and filters.
        let [sx, sy, sw] = Self::search_rect(w);
        (self.search.x, self.search.y, self.search.width, self.search.height) = (sx, sy, sw, 20.0);
        self.search.draw(batch, atlas, None);
        let fx = sx + sw + 2.0;
        ui::button(
            batch,
            atlas,
            fx,
            sy,
            20.0,
            20.0,
            &Text::literal("≡"),
            true,
            ui::hovered(cursor, fx, sy, 20.0, 20.0),
        );
        let rows = shown(installed, &self.search.value, self.sort, self.libraries);
        if self.filters {
            let [a, b] = self.filter_buttons(atlas, w);
            let (sort, libraries) = (self.sort_label(), self.libraries_label());
            for ([x, y, bw, bh], label) in [(a, sort), (b, libraries)] {
                let hover = ui::hovered(cursor, x, y, bw, bh);
                ui::button(batch, atlas, x, y, bw, bh, &label, true, hover);
            }
        } else {
            let libraries = rows
                .iter()
                .filter(|&&i| LIBRARIES.contains(&installed[i].id.as_str()))
                .count();
            let line = lang::get_or("horizon.mod_menu.count", "Showing %s mods (%s libraries)")
                .replacen("%s", &(rows.len() - libraries).to_string(), 1)
                .replacen("%s", &libraries.to_string(), 1);
            batch.text(atlas, &line, sx, 52.0, u32::MAX, true);
        }
        // The list.
        let fit = Self::rows_fit(h);
        self.first = self.first.min(rows.len().saturating_sub(fit));
        if self.selected.is_none_or(|s| !rows.contains(&s)) {
            self.selected = rows.first().copied();
        }
        for (k, &i) in rows.iter().enumerate().skip(self.first).take(fit) {
            let y = LIST_Y + (k - self.first) as f32 * ROW;
            let row = &installed[i];
            if self.selected == Some(i) {
                batch.fill(atlas, 2.0, y - 2.0, pane - 4.0, ROW, light);
                batch.fill(atlas, 3.0, y - 1.0, pane - 6.0, ROW - 2.0, rgba(0, 0, 0, 255));
            }
            let (name, _) = name_and_version(&row.title);
            Self::icon(batch, atlas, 5.0, y, name);
            let tx = 5.0 + ICON + 3.0;
            let room = pane - tx - 6.0;
            let trimmed = crate::split_profiles::wrap(name, room, |t| {
                HudBatch::text_width(atlas, t)
            });
            batch.text(atlas, trimmed.first().map_or(name, String::as_str), tx, y + 1.0, u32::MAX, false);
            let summary = row.problem.as_deref().unwrap_or(&row.description);
            let lines = crate::split_profiles::wrap(summary, room - 4.0, |t| {
                HudBatch::text_width(atlas, t)
            });
            for (l, line) in lines.iter().take(2).enumerate() {
                batch.text(atlas, line, tx + 4.0, y + 10.0 + l as f32 * 9.0, grey, false);
            }
        }
        // The selected mod.
        if let Some(mod_row) = self.selected.and_then(|i| installed.get(i).map(|m| (i, m))) {
            let (i, row) = mod_row;
            let x = w - pane;
            let (name, version) = name_and_version(&row.title);
            Self::icon(batch, atlas, x, RIGHT_PANE_Y, name);
            batch.text(atlas, name, x + 36.0, RIGHT_PANE_Y + 1.0, u32::MAX, true);
            if LIBRARIES.contains(&row.id.as_str()) {
                let tag = tr("horizon.mod_menu.library", "Library");
                let bx = x + 36.0 + HudBatch::text_width(atlas, name).ceil() + 4.0;
                let tw = HudBatch::text_width(atlas, &tag).ceil() + 6.0;
                batch.fill(atlas, bx, RIGHT_PANE_Y, tw, 11.0, rgba(60, 60, 120, 255));
                batch.text(atlas, &tag, bx + 3.0, RIGHT_PANE_Y + 1.0, u32::MAX, false);
            }
            batch.text(atlas, version, x + 36.0, RIGHT_PANE_Y + 11.0, grey, true);
            batch.text(atlas, &row.id, x + 36.0, RIGHT_PANE_Y + 21.0, grey, true);
            let [toggle, settings] = Self::detail_buttons(w);
            let on = mods.is_on(i);
            let label = Text::literal(format!(
                "{}: {}",
                tr("horizon.mod_menu.enabled", "Enabled"),
                lang::get_or(if on { "options.on" } else { "options.off" }, if on { "ON" } else { "OFF" })
            ));
            let dead = row.dev || row.id.is_empty();
            let hover = ui::hovered(cursor, toggle[0], toggle[1], toggle[2], toggle[3]);
            ui::button(batch, atlas, toggle[0], toggle[1], toggle[2], toggle[3], &label, !dead, hover);
            if row.native.as_ref().is_some_and(|p| !p.keys.is_empty() || !p.buttons.is_empty()) {
                let hover = ui::hovered(cursor, settings[0], settings[1], settings[2], settings[3]);
                let label = Text::literal(tr("horizon.mods.settings", "Settings..."));
                ui::button(batch, atlas, settings[0], settings[1], settings[2], settings[3], &label, true, hover);
            }
            let body = row.problem.as_deref().unwrap_or(&row.description);
            let lines = crate::split_profiles::wrap(body, pane - 4.0, |t| {
                HudBatch::text_width(atlas, t)
            });
            let mut y = RIGHT_PANE_Y + 60.0;
            for line in lines {
                if y > h - 40.0 {
                    break;
                }
                batch.text(atlas, &line, x, y, light, false);
                y += 10.0;
            }
        }
        // Mods folder and Done.
        let [folder, done] = Self::bottom(w, h);
        for ([x, y, bw, bh], label) in [
            (folder, tr("horizon.mods.open_folder", "Open Mods Folder")),
            (done, tr("gui.done", "Done")),
        ] {
            let hover = ui::hovered(cursor, x, y, bw, bh);
            ui::button(batch, atlas, x, y, bw, bh, &Text::literal(label), true, hover);
        }
    }

    /// A mod's icon: Mod Menu falls back to a plain one; here its initial on a colour of its name.
    fn icon(batch: &mut HudBatch, atlas: &HudAtlas, x: f32, y: f32, name: &str) {
        let hash = name.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
        let part = |s: u32| 60 + ((hash >> s) & 0x7f) as u8;
        batch.fill(atlas, x, y, ICON, ICON, rgba(part(0), part(8), part(16), 255));
        let initial: String = name.chars().take(1).collect();
        let tw = HudBatch::text_width(atlas, &initial);
        batch.text(atlas, &initial, x + (ICON - tw) / 2.0, y + 12.0, u32::MAX, true);
    }

    /// A press at `(x, y)` on a screen `w` x `h`.
    pub fn click(&mut self, mods: &ModList, atlas: &HudAtlas, (x, y): (f32, f32), (w, h): (f32, f32)) -> Click {
        let installed = &mods.installed;
        let on = |r: [f32; 4]| ui::hovered((x, y), r[0], r[1], r[2], r[3]);
        let [folder, done] = Self::bottom(w, h);
        if on(folder) {
            return Click::Folder;
        }
        if on(done) {
            return Click::Done;
        }
        let [sx, sy, sw] = Self::search_rect(w);
        self.search.focused = ui::hovered((x, y), sx, sy, sw, 20.0);
        if self.search.focused {
            self.search.click(atlas, x, false, false);
            return Click::Nothing;
        }
        if ui::hovered((x, y), sx + sw + 2.0, sy, 20.0, 20.0) {
            self.filters = !self.filters;
            return Click::Pressed;
        }
        if self.filters {
            let [a, b] = self.filter_buttons(atlas, w);
            if on(a) {
                self.sort = match self.sort {
                    Sort::Ascending => Sort::Descending,
                    Sort::Descending => Sort::Ascending,
                };
                return Click::Pressed;
            }
            if on(b) {
                self.libraries = !self.libraries;
                self.first = 0;
                return Click::Pressed;
            }
        }
        if let Some(i) = self.selected.filter(|&i| i < installed.len()) {
            let [toggle, settings] = Self::detail_buttons(w);
            if on(toggle) && !installed[i].dev && !installed[i].id.is_empty() {
                return Click::Toggle(i);
            }
            if on(settings)
                && installed[i]
                    .native
                    .as_ref()
                    .is_some_and(|p| !p.keys.is_empty() || !p.buttons.is_empty())
            {
                return Click::Settings(i);
            }
        }
        if x < Self::pane(w) && y >= LIST_Y && y < h - 36.0 {
            let k = ((y - LIST_Y) / ROW) as usize + self.first;
            let rows = shown(installed, &self.search.value, self.sort, self.libraries);
            if let Some(&i) = rows.get(k) {
                self.selected = Some(i);
                return Click::Pressed;
            }
        }
        Click::Nothing
    }

    /// The wheel over the list, by rows.
    pub fn scroll(&mut self, amount: f32) {
        self.first = if amount > 0.0 {
            self.first.saturating_sub(1)
        } else {
            self.first + 1
        };
    }

    pub fn type_text(&mut self, text: &str) {
        if self.search.focused {
            self.search.insert_text(text);
            self.first = 0;
        }
    }

    /// A key: true when the search took it.
    pub fn key(&mut self, key: winit::keyboard::KeyCode, text: Option<&str>) -> bool {
        if !self.search.focused || key == winit::keyboard::KeyCode::Escape {
            return false;
        }
        if !self.search.key(key, false, false)
            && let Some(t) = text
        {
            let typed: String = t.chars().filter(|c| !c.is_control()).collect();
            self.search.insert_text(&typed);
        }
        self.first = 0;
        true
    }

    /// Ctrl+A/C/V/X in the search; false when it hasn't the focus.
    pub fn shortcut(
        &mut self,
        shortcut: crate::clipboard::EditShortcut,
        clipboard: &mut dyn crate::clipboard::Clipboard,
    ) -> bool {
        self.search.focused && self.search.shortcut(shortcut, clipboard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(id: &str, title: &str) -> Installed {
        Installed {
            id: id.into(),
            title: title.into(),
            description: format!("About {id}"),
            problem: None,
            dev: false,
            native: None,
        }
    }

    #[test]
    fn the_list_searches_sorts_and_hides_libraries() {
        let mods = [
            installed("physics", "Physics 0.1.0"),
            installed("oxyra_api", "Oxyra API 0.2.1"),
            installed("better_f3", "BetterF3 0.1.1"),
        ];
        assert_eq!(shown(&mods, "", Sort::Ascending, true), [2, 1, 0]);
        assert_eq!(shown(&mods, "", Sort::Descending, true), [0, 1, 2]);
        assert_eq!(shown(&mods, "", Sort::Ascending, false), [2, 0]);
        assert_eq!(shown(&mods, "PHYS", Sort::Ascending, true), [0]);
        assert_eq!(name_and_version("Oxyra API 0.2.1"), ("Oxyra API", "0.2.1"));
    }
}
