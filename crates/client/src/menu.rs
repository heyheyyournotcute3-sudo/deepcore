//! Main menu: host a game, or join one by IP.

use bevy::prelude::*;
use bevy::text::FontSize;
use bevy::ui_widgets::{Activate, Button};

use deepcore_shared::protocol::DEFAULT_PORT;

use crate::GameState;
use crate::net::{ConnectionOptions, ConnectionState};

/// The menu root, despawned when play starts.
#[derive(Component)]
pub struct MenuRoot;

/// The address input box.
#[derive(Component)]
pub struct AddressField(pub String);

/// The name input box.
#[derive(Component)]
pub struct NameField(pub String);

/// The status line.
#[derive(Component)]
pub struct StatusLine;

/// Builds the menu UI. If `--connect` was passed, joins immediately instead.
pub fn setup_menu(mut commands: Commands, mut options: ResMut<ConnectionOptions>) {
    if let Some((address, name)) = crate::parse_connect_args() {
        options.address = address;
        options.name = name.clone();
        options.host = false;
        info!("auto-connecting to {address} as {name}");
        commands.insert_resource(MenuState { started: true });
        return;
    }

    let (default_address, default_name) = (format!("127.0.0.1:{DEFAULT_PORT}"), "Miner".to_owned());

    commands
        .spawn((
            MenuRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(14.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.05, 0.06, 0.08, 0.92)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("DEEPCORE"),
                TextFont { font_size: FontSize::Px(64.0), ..default() },
                TextColor(Color::srgb(0.85, 0.92, 1.0)),
            ));
            parent.spawn((
                Text::new("dig deeper · bring ore home · build machines · go nuclear"),
                TextFont { font_size: FontSize::Px(18.0), ..default() },
                TextColor(Color::srgb(0.6, 0.7, 0.75)),
            ));

            // Host button.
            parent
                .spawn((
                    Button,
                    Node {
                        width: Val::Px(320.0),
                        height: Val::Px(48.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.18, 0.32, 0.24)),
                ))
                .with_children(|button| {
                    button.spawn((
                        Text::new("Host game"),
                        TextFont { font_size: FontSize::Px(22.0), ..default() },
                        TextColor(Color::WHITE),
                    ));
                })
                .insert(HostButton);

            // Join button.
            parent
                .spawn((
                    Button,
                    Node {
                        width: Val::Px(320.0),
                        height: Val::Px(48.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.20, 0.26, 0.36)),
                ))
                .with_children(|button| {
                    button.spawn((
                        Text::new("Join by IP"),
                        TextFont { font_size: FontSize::Px(22.0), ..default() },
                        TextColor(Color::WHITE),
                    ));
                })
                .insert(JoinButton);

            parent.spawn((
                Text::new(format!("address: {default_address}")),
                TextFont { font_size: FontSize::Px(16.0), ..default() },
                TextColor(Color::srgb(0.7, 0.75, 0.8)),
                AddressField(default_address),
            ));
            parent.spawn((
                Text::new(format!("name: {default_name}")),
                TextFont { font_size: FontSize::Px(16.0), ..default() },
                TextColor(Color::srgb(0.7, 0.75, 0.8)),
                NameField(default_name),
            ));
            parent.spawn((
                Text::new(""),
                TextFont { font_size: FontSize::Px(16.0), ..default() },
                TextColor(Color::srgb(1.0, 0.6, 0.4)),
                StatusLine,
            ));
            parent.spawn((
                Text::new("Command line: --connect <ip:port> --name <name>"),
                TextFont { font_size: FontSize::Px(13.0), ..default() },
                TextColor(Color::srgb(0.45, 0.5, 0.55)),
            ));
        });

    commands.insert_resource(MenuState::default());
}

/// The menu's widget queries, bundled so the activation observer stays readable.
#[derive(bevy::ecs::system::SystemParam)]
pub struct MenuWidgets<'w, 's> {
    /// The address field.
    pub address: Query<'w, 's, &'static AddressField>,
    /// The name field.
    pub name: Query<'w, 's, &'static NameField>,
    /// The status line.
    pub status: Query<'w, 's, &'static mut Text, With<StatusLine>>,
    /// The menu root entity.
    pub roots: Query<'w, 's, Entity, With<MenuRoot>>,
    /// Host buttons.
    pub hosts: Query<'w, 's, (), With<HostButton>>,
    /// Join buttons.
    pub joins: Query<'w, 's, (), With<JoinButton>>,
}

/// Exclusive system that inserts the renet resources once the address is known.
pub fn start_network(world: &mut World) {
    crate::net::start_client(world);
}

/// Marker for the host button.
#[derive(Component)]
pub struct HostButton;

/// Marker for the join button.
#[derive(Component)]
pub struct JoinButton;

/// Menu bookkeeping.
#[derive(Resource, Default)]
pub struct MenuState {
    /// True once the player has picked host or join.
    pub started: bool,
}

/// Starts the network stack and switches to the game once the menu has started it.
///
/// Running this as a normal system (rather than a cached exclusive command) means the state
/// machine is fully initialised before it is asked to transition.
pub fn drive_startup(
    mut menu_state: ResMut<MenuState>,
    mut next_state: ResMut<NextState<GameState>>,
    mut connection: ResMut<ConnectionState>,
    mut commands: Commands,
) {
    if !menu_state.started {
        return;
    }
    menu_state.started = false;
    *connection = ConnectionState::Connecting;
    next_state.set(GameState::Playing);
    commands.run_system_cached(start_network);
}

/// Reacts to the menu buttons being activated.
pub fn menu_actions(
    activate: On<Activate>,
    mut commands: Commands,
    mut next_state: ResMut<NextState<GameState>>,
    mut options: ResMut<ConnectionOptions>,
    mut menu_state: ResMut<MenuState>,
    mut connection: ResMut<ConnectionState>,
    widgets: MenuWidgets,
) {
    let MenuWidgets { address, name, mut status, roots, hosts, joins } = widgets;
    if menu_state.started {
        return;
    }
    let entity = activate.entity;
    let address_text = address.iter().next().map(|field| field.0.clone()).unwrap_or_default();
    let name_text = name.iter().next().map(|field| field.0.clone()).unwrap_or_default();

    if hosts.contains(entity) {
        // Host: run a server in this process and connect to it over the loopback.
        options.host = true;
        options.name = name_text.clone();
        options.address =
            SocketAddrLike::from(([127, 0, 0, 1], deepcore_shared::protocol::DEFAULT_PORT));
        info!("hosting on {}", options.address);
        start(&mut commands, &mut next_state, &mut menu_state, &mut connection, &roots);
        return;
    }
    if joins.contains(entity) {
        match address_text.parse() {
            Ok(address) => {
                options.host = false;
                options.name = name_text.clone();
                options.address = address;
                info!("joining {}", options.address);
                start(&mut commands, &mut next_state, &mut menu_state, &mut connection, &roots);
            }
            Err(_) => {
                if let Ok(mut text) = status.single_mut() {
                    **text = format!("`{address_text}` is not an ip:port address");
                }
            }
        }
    }
}

type SocketAddrLike = std::net::SocketAddr;

fn start(
    commands: &mut Commands,
    next_state: &mut NextState<GameState>,
    menu_state: &mut MenuState,
    connection: &mut ConnectionState,
    roots: &Query<Entity, With<MenuRoot>>,
) {
    menu_state.started = true;
    *connection = ConnectionState::Connecting;
    for entity in roots {
        commands.entity(entity).despawn();
    }
    next_state.set(GameState::Playing);
    // The network stack starts from `start_network`, which runs as an exclusive system so it
    // can insert the renet resources.
    commands.run_system_cached(start_network);
}

/// Shows connection progress on the status line while the game loads.
pub fn menu_status(
    connection: Res<ConnectionState>,
    mut status: Query<&mut Text, With<StatusLine>>,
) {
    let Ok(mut text) = status.single_mut() else {
        return;
    };
    let message = match &*connection {
        ConnectionState::Disconnected => "not connected",
        ConnectionState::Connecting => "connecting...",
        ConnectionState::Loading => "loading the mine...",
        ConnectionState::Playing => "in game",
        ConnectionState::Rejected(reason) => reason.as_str(),
    };
    if **text != message {
        **text = message.to_owned();
    }
}
