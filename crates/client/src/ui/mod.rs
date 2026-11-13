//! Inventory, crafting and research screens, drawn with egui.
//!
//! Immediate mode is the right tool here: the inventory, the workbench recipe list and the tech
//! tree are all dense, data-driven panels that would be a lot of retained UI code otherwise.
//! The server owns the truth; this module renders snapshots and sends intents.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use deepcore_shared::content::ContentRegistry;
use deepcore_shared::content::registry::RecipeId;
use deepcore_shared::inventory::{HOTBAR_SLOTS, Slot};
use deepcore_shared::protocol::{MoveItems, ResearchDeposit, SelectHotbar};
use deepcore_shared::tech::TechState;

use crate::input::CursorGrab;
use crate::net::{ConnectionState, Notices};
use crate::state::{ClientInventory, ClientTech, OpenMachine, predict_craft};

/// Which screen is open.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpenScreen {
    /// No panel.
    #[default]
    None,
    /// Inventory and backpack.
    Inventory,
    /// Hand crafting list.
    Crafting,
    /// Research desk / tech tree.
    Research,
}

/// True once egui has run at least once, so fonts and styles exist.
#[derive(Resource, Default)]
pub struct EguiReady(pub bool);

/// Marks egui as ready once the first full frame (including egui's `PostUpdate` pass) has run.
pub fn mark_egui_ready(mut ready: ResMut<EguiReady>, mut frames: Local<u32>) {
    *frames += 1;
    if *frames >= 2 {
        ready.0 = true;
    }
}

/// Registers egui.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default());
        app.init_resource::<OpenScreen>();
        app.init_resource::<OpenMachine>();
        app.init_resource::<ClientTech>();
        app.init_resource::<ClientInventory>();
        app.init_resource::<UiSelection>();
        app.init_resource::<EguiReady>();
        // The UI writes these intents; registering them here means the panels work even before
        // the network layer has been started (e.g. in the menu).
        app.add_message::<MoveItems>()
            .add_message::<SelectHotbar>()
            .add_message::<ResearchDeposit>()
            .add_message::<deepcore_shared::protocol::CraftRequest>()
            .add_message::<deepcore_shared::protocol::Interact>();
        // bevy_egui runs its UI systems in the dedicated `EguiPrimaryContextPass`, which is
        // the only place a `Context` may be touched (see the crate's own examples).
        app.add_systems(
            EguiPrimaryContextPass,
            (
                toggle_screens,
                inventory_panel,
                crafting_panel,
                machine_panel,
                research_panel,
                hud_panel,
                objective_panel,
                mark_egui_ready,
            ),
        );
    }
}

/// UI-only state: the currently picked-up stack and the selected recipe.
#[derive(Resource, Default)]
pub struct UiSelection {
    /// The slot index held by the cursor.
    pub held: Option<usize>,
    /// The recipe selected in the crafting list.
    pub recipe: Option<RecipeId>,
}

/// Opens and closes the screens and releases the mouse while they are open.
pub fn toggle_screens(
    keys: Res<ButtonInput<KeyCode>>,
    mut screen: ResMut<OpenScreen>,
    mut grab: ResMut<CursorGrab>,
    mut cursors: Query<&mut bevy::window::CursorOptions>,
    state: Res<ConnectionState>,
) {
    if !matches!(*state, ConnectionState::Playing) {
        return;
    }
    if keys.just_pressed(KeyCode::KeyE) || keys.just_pressed(KeyCode::Tab) {
        *screen =
            if *screen == OpenScreen::Inventory { OpenScreen::None } else { OpenScreen::Inventory };
    }
    if keys.just_pressed(KeyCode::KeyC) {
        *screen =
            if *screen == OpenScreen::Crafting { OpenScreen::None } else { OpenScreen::Crafting };
    }
    if keys.just_pressed(KeyCode::KeyT) {
        *screen =
            if *screen == OpenScreen::Research { OpenScreen::None } else { OpenScreen::Research };
    }

    // Release the cursor while a panel is open so it can be used.
    let wants_cursor = *screen != OpenScreen::None;
    if wants_cursor != !grab.grabbed {
        grab.grabbed = !wants_cursor;
        if let Ok(mut cursor) = cursors.single_mut() {
            cursor.visible = wants_cursor;
            cursor.grab_mode = if wants_cursor {
                bevy::window::CursorGrabMode::None
            } else {
                bevy::window::CursorGrabMode::Locked
            };
        }
    }
}

/// Draws a slot button with the item's colour, name and count.
fn slot_widget(
    ui: &mut egui::Ui,
    slot: &Slot,
    registry: &ContentRegistry,
    size: f32,
    highlighted: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag());
    let painter = ui.painter();
    let background = if highlighted {
        egui::Color32::from_rgb(70, 80, 60)
    } else {
        egui::Color32::from_rgb(34, 36, 40)
    };
    painter.rect_filled(rect, 4.0, background);
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 74, 80)),
        egui::StrokeKind::Inside,
    );

    if let Some(item) = slot.item
        && slot.count > 0
    {
        let entry = registry.item(item);
        let color = entry.map(|entry| entry.def.color).unwrap_or((0.7, 0.7, 0.7));
        let rgb = egui::Color32::from_rgb(
            (color.0.clamp(0.0, 1.0) * 255.0) as u8,
            (color.1.clamp(0.0, 1.0) * 255.0) as u8,
            (color.2.clamp(0.0, 1.0) * 255.0) as u8,
        );
        let inner = rect.shrink(size * 0.18);
        painter.rect_filled(inner, 3.0, rgb);
        // Count, bottom right.
        if slot.count > 1 {
            painter.text(
                rect.right_bottom() + egui::vec2(-3.0, -2.0),
                egui::Align2::RIGHT_BOTTOM,
                slot.count.to_string(),
                egui::FontId::proportional(size * 0.30),
                egui::Color32::WHITE,
            );
        }
    }

    // Hover text with the item name.
    let name = slot
        .item
        .filter(|_| slot.count > 0)
        .map(|item| registry.item_name(item).to_owned())
        .unwrap_or_else(|| "Empty".to_owned());
    response.on_hover_text(name)
}

/// The inventory and backpack panel.
pub fn inventory_panel(
    mut contexts: EguiContexts,
    ready: Res<EguiReady>,
    screen: Res<OpenScreen>,
    mut inventory: ResMut<ClientInventory>,
    mut selection: ResMut<UiSelection>,
    registry: Res<ContentRegistry>,
    mut moves: MessageWriter<MoveItems>,
    mut select: MessageWriter<SelectHotbar>,
) {
    if *screen != OpenScreen::Inventory || !ready.0 {
        selection.held = None;
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let ctx = ctx.clone();

    egui::Window::new("Inventory")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(&ctx, |ui| {
            ui.label("Hotbar");
            ui.horizontal(|ui| {
                for index in 0..HOTBAR_SLOTS {
                    let slot = *inventory.inventory.slot(index).unwrap_or(&Slot::EMPTY);
                    let selected = index as u8 == inventory.selected;
                    let response = slot_widget(ui, &slot, &registry, 44.0, selected);
                    if response.clicked() {
                        inventory.selected = index as u8;
                        select.write(SelectHotbar { slot: index as u8 });
                    }
                    handle_slot_interaction(
                        &response,
                        index,
                        &mut selection,
                        &mut moves,
                        &registry,
                    );
                }
            });

            ui.add_space(6.0);
            ui.label("Backpack");
            egui::Grid::new("backpack").spacing([4.0, 4.0]).show(ui, |ui| {
                for index in HOTBAR_SLOTS..inventory.inventory.len() {
                    let slot = *inventory.inventory.slot(index).unwrap_or(&Slot::EMPTY);
                    let response = slot_widget(ui, &slot, &registry, 40.0, false);
                    handle_slot_interaction(
                        &response,
                        index,
                        &mut selection,
                        &mut moves,
                        &registry,
                    );
                    if (index - HOTBAR_SLOTS + 1).is_multiple_of(9) {
                        ui.end_row();
                    }
                }
            });

            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Left click picks a stack up, right click splits, shift-click moves",
                )
                .small()
                .color(egui::Color32::GRAY),
            );
        });
}

/// Turns slot clicks into `MoveItems` intents.
fn handle_slot_interaction(
    response: &egui::Response,
    index: usize,
    selection: &mut UiSelection,
    moves: &mut MessageWriter<MoveItems>,
    _registry: &ContentRegistry,
) {
    if response.clicked() {
        match selection.held {
            Some(from) => {
                moves.write(MoveItems {
                    from_container: 0,
                    from_slot: from as u16,
                    to_container: 0,
                    to_slot: index as u16,
                    count: u32::MAX,
                });
                selection.held = None;
            }
            None => selection.held = Some(index),
        }
    }
    if response.secondary_clicked() {
        moves.write(MoveItems {
            from_container: 0,
            from_slot: index as u16,
            to_container: 0,
            to_slot: ((index + HOTBAR_SLOTS) % deepcore_shared::inventory::PLAYER_SLOTS) as u16,
            count: 1,
        });
    }
}

/// The hand-crafting panel.
pub fn crafting_panel(
    mut contexts: EguiContexts,
    ready: Res<EguiReady>,
    screen: Res<OpenScreen>,
    mut selection: ResMut<UiSelection>,
    inventory: Res<ClientInventory>,
    tech: Res<ClientTech>,
    registry: Res<ContentRegistry>,
    mut crafts: MessageWriter<deepcore_shared::protocol::CraftRequest>,
    mut notices: ResMut<Notices>,
) {
    if *screen != OpenScreen::Crafting || !ready.0 {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let ctx = ctx.clone();
    let unlocked = tech.state.available_recipes(&registry);

    egui::Window::new("Crafting")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(520.0)
        .show(&ctx, |ui| {
            let recipes: Vec<RecipeId> = registry
                .hand_recipes()
                .iter()
                .copied()
                .filter(|recipe| unlocked.contains(recipe))
                .collect();

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(240.0);
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        for recipe in &recipes {
                            let Some(entry) = registry.recipe(*recipe) else {
                                continue;
                            };
                            let output = entry.outputs.first().map(|(item, count)| {
                                format!("{} x{}", registry.item_name(*item), count)
                            });
                            let label = output.unwrap_or_else(|| entry.def.id.clone());
                            let can = deepcore_shared::crafting::can_craft(
                                &registry,
                                &inventory.inventory,
                                *recipe,
                            );
                            let text = if can {
                                egui::RichText::new(label).color(egui::Color32::LIGHT_GREEN)
                            } else {
                                egui::RichText::new(label).color(egui::Color32::GRAY)
                            };
                            if ui
                                .selectable_label(selection.recipe == Some(*recipe), text)
                                .clicked()
                            {
                                selection.recipe = Some(*recipe);
                            }
                        }
                    });
                });

                ui.separator();

                ui.vertical(|ui| {
                    ui.set_width(240.0);
                    match selection.recipe {
                        Some(recipe) => {
                            let Some(entry) = registry.recipe(recipe) else {
                                return;
                            };
                            ui.heading(
                                entry
                                    .outputs
                                    .first()
                                    .map(|(item, _)| registry.item_name(*item))
                                    .unwrap_or("Recipe"),
                            );
                            ui.add_space(4.0);
                            ui.label("Needs:");
                            for (item, count) in &entry.inputs {
                                let have = inventory.inventory.count_of(*item);
                                let color = if have >= *count {
                                    egui::Color32::LIGHT_GREEN
                                } else {
                                    egui::Color32::from_rgb(240, 120, 100)
                                };
                                ui.label(
                                    egui::RichText::new(format!(
                                        "  {} x{} (have {have})",
                                        registry.item_name(*item),
                                        count
                                    ))
                                    .color(color),
                                );
                            }
                            ui.add_space(6.0);
                            let ready =
                                predict_craft(&registry, &inventory.inventory, recipe).is_ok();
                            if ui
                                .add_enabled(
                                    ready,
                                    egui::Button::new("Craft").min_size(egui::vec2(120.0, 28.0)),
                                )
                                .clicked()
                            {
                                crafts.write(deepcore_shared::protocol::CraftRequest {
                                    recipe: recipe.0,
                                    count: 1,
                                });
                            }
                            if !ready {
                                ui.label(
                                    egui::RichText::new("Not enough materials")
                                        .small()
                                        .color(egui::Color32::GRAY),
                                );
                            }
                        }
                        None => {
                            ui.label("Pick a recipe on the left");
                        }
                    }
                    let _ = &mut notices;
                });
            });
        });
}

/// The machine panel (furnace, kiln, anvil, ...).
pub fn machine_panel(
    mut contexts: EguiContexts,
    ready: Res<EguiReady>,
    machine: Res<OpenMachine>,
    mut moves: MessageWriter<MoveItems>,
    registry: Res<ContentRegistry>,
) {
    let Some(entity) = machine.entity else {
        return;
    };
    if !ready.0 {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let ctx = ctx.clone();

    egui::Window::new("Machine")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(&ctx, |ui| {
            ui.label(format!(
                "at {} {} {}",
                machine.position[0], machine.position[1], machine.position[2]
            ));
            ui.add_space(4.0);

            if !machine.fuel.is_empty() {
                ui.label("Fuel");
                ui.horizontal(|ui| {
                    for (index, slot) in machine.fuel.iter().enumerate() {
                        let response = slot_widget(ui, slot, &registry, 40.0, false);
                        if response.clicked() {
                            let input_len = machine.inputs.len() as u16;
                            moves.write(MoveItems {
                                from_container: 0,
                                from_slot: 0,
                                to_container: entity,
                                to_slot: input_len + index as u16,
                                count: u32::MAX,
                            });
                        }
                    }
                });
            }

            ui.label("Inputs");
            ui.horizontal(|ui| {
                for (index, slot) in machine.inputs.iter().enumerate() {
                    let response = slot_widget(ui, slot, &registry, 40.0, false);
                    if response.clicked() {
                        moves.write(MoveItems {
                            from_container: entity,
                            from_slot: index as u16,
                            to_container: 0,
                            to_slot: 0,
                            count: u32::MAX,
                        });
                    }
                }
            });

            ui.label("Outputs");
            ui.horizontal(|ui| {
                for (index, slot) in machine.outputs.iter().enumerate() {
                    let response = slot_widget(ui, slot, &registry, 40.0, false);
                    if response.clicked() {
                        let offset = machine.inputs.len() as u16;
                        moves.write(MoveItems {
                            from_container: entity,
                            from_slot: offset + index as u16,
                            to_container: 0,
                            to_slot: 0,
                            count: u32::MAX,
                        });
                    }
                }
            });

            ui.add_space(4.0);
            ui.add(
                egui::ProgressBar::new(machine.progress)
                    .desired_width(180.0)
                    .text(format!("{:.0}%", machine.progress * 100.0)),
            );
            if let Some(idle) = &machine.idle {
                ui.label(egui::RichText::new(idle).color(egui::Color32::from_rgb(240, 160, 90)));
            }
        });
}

/// The tech tree panel.
pub fn research_panel(
    mut contexts: EguiContexts,
    ready: Res<EguiReady>,
    screen: Res<OpenScreen>,
    tech: Res<ClientTech>,
    registry: Res<ContentRegistry>,
    inventory: Res<ClientInventory>,
    mut deposits: MessageWriter<ResearchDeposit>,
    mut selection: ResMut<UiSelection>,
) {
    if *screen != OpenScreen::Research || !ready.0 {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let ctx = ctx.clone();

    egui::Window::new("Research")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(700.0)
        .show(&ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Era {}", tech.state.era(&registry)));
                ui.label(format!("{} nodes researched", tech.state.unlocked_count()));
            });
            ui.separator();

            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                for era in 0..=5u8 {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("Era {era}"))
                            .strong()
                            .color(egui::Color32::from_rgb(150, 190, 230)),
                    );
                    for (index, node) in registry.tech_nodes().iter().enumerate() {
                        if node.def.era != era {
                            continue;
                        }
                        let id = deepcore_shared::content::registry::TechId(index as u16);
                        let unlocked = tech.state.is_unlocked(id);
                        let available = tech.state.is_available(&registry, id);
                        ui.horizontal(|ui| {
                            let (marker, color) = if unlocked {
                                ("[x]", egui::Color32::LIGHT_GREEN)
                            } else if available {
                                ("[ ]", egui::Color32::from_rgb(230, 220, 150))
                            } else {
                                ("[ ]", egui::Color32::DARK_GRAY)
                            };
                            ui.label(egui::RichText::new(marker).color(color));
                            ui.label(egui::RichText::new(&node.def.name).color(color));

                            if available {
                                let mut all_delivered = true;
                                for (item, count) in &node.cost {
                                    let have = inventory.inventory.count_of(*item)
                                        + tech.state.delivered(id, *item);
                                    if have < *count {
                                        all_delivered = false;
                                    }
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} {}/{}",
                                            registry.item_name(*item),
                                            have,
                                            count
                                        ))
                                        .small()
                                        .color(
                                            if have >= *count {
                                                egui::Color32::LIGHT_GREEN
                                            } else {
                                                egui::Color32::GRAY
                                            },
                                        ),
                                    );
                                }
                                if ui.button("Deposit").clicked() {
                                    deposits.write(ResearchDeposit { node: index as u16 });
                                }
                                if all_delivered && !node.cost.is_empty() {
                                    ui.label(
                                        egui::RichText::new("ready")
                                            .small()
                                            .color(egui::Color32::LIGHT_GREEN),
                                    );
                                }
                            }
                        });
                    }
                }
            });
            ui.separator();
            ui.label(
                egui::RichText::new("Deliver the listed materials to finish a node.")
                    .small()
                    .color(egui::Color32::GRAY),
            );
            let _ = &mut selection;
        });
}

/// The always-on HUD: hotbar, notices and connection state.
pub fn hud_panel(
    mut contexts: EguiContexts,
    ready: Res<EguiReady>,
    mut logged: Local<bool>,
    inventory: Res<ClientInventory>,
    notices: Res<Notices>,
    registry: Res<ContentRegistry>,
    state: Res<ConnectionState>,
    mut select: MessageWriter<SelectHotbar>,
) {
    if !ready.0 {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let _ = &mut logged;
    let ctx = ctx.clone();

    if !matches!(*state, ConnectionState::Playing) {
        let message = match &*state {
            ConnectionState::Connecting => "Connecting...",
            ConnectionState::Loading => "Loading the mine...",
            ConnectionState::Rejected(reason) => reason.as_str(),
            ConnectionState::Disconnected => "Disconnected",
            _ => "",
        };
        if !message.is_empty() {
            egui::Area::new("connection".into())
                .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 40.0))
                .show(&ctx, |ui| {
                    ui.label(egui::RichText::new(message).size(22.0).color(egui::Color32::WHITE));
                });
        }
        return;
    }

    // Hotbar along the bottom.
    egui::Area::new("hotbar".into())
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -12.0))
        .show(&ctx, |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_black_alpha(140))
                .inner_margin(6.0)
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for index in 0..HOTBAR_SLOTS {
                            let slot = *inventory.inventory.slot(index).unwrap_or(&Slot::EMPTY);
                            let selected = index as u8 == inventory.selected;
                            let response = slot_widget(ui, &slot, &registry, 40.0, selected);
                            if response.clicked() {
                                select.write(SelectHotbar { slot: index as u8 });
                            }
                        }
                    });
                });
        });

    // Notices above the hotbar.
    if !notices.latest.is_empty() {
        egui::Area::new("notice".into())
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -80.0))
            .show(&ctx, |ui| {
                ui.label(
                    egui::RichText::new(&notices.latest)
                        .size(20.0)
                        .color(egui::Color32::from_rgb(255, 210, 140)),
                );
            });
    }

    // Held item name, bottom right.
    if let Some(item) = inventory.held() {
        egui::Area::new("held".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -70.0))
            .show(&ctx, |ui| {
                ui.label(
                    egui::RichText::new(registry.item_name(item))
                        .size(16.0)
                        .color(egui::Color32::LIGHT_GRAY),
                );
            });
    }
}

/// Draws the objective line under the crosshair: what the team should do next.
pub fn objective_panel(
    mut contexts: EguiContexts,
    ready: Res<EguiReady>,
    tech: Res<ClientTech>,
    registry: Res<ContentRegistry>,
    state: Res<ConnectionState>,
) {
    if !matches!(*state, ConnectionState::Playing) || !ready.0 {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let ctx = ctx.clone();
    if let Some(objective) = next_objective(&registry, &tech.state) {
        egui::Area::new("objective".into())
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 18.0))
            .show(&ctx, |ui| {
                ui.label(
                    egui::RichText::new(objective)
                        .size(16.0)
                        .color(egui::Color32::from_rgb(200, 220, 250)),
                );
            });
    }
}

/// A snapshot of what the tech tree still needs, for the objective list.
pub fn next_objective(registry: &ContentRegistry, tech: &TechState) -> Option<String> {
    let available = tech.available_nodes(registry);
    available.first().map(|node| {
        registry.tech(*node).map(|entry| format!("Research {}", entry.def.name)).unwrap_or_default()
    })
}
