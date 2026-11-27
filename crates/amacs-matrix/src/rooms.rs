//! The sync with the server, and the rooms that it brings.
//!
//! The SDK's `SyncService` runs the sync: one connection for the list of
//! rooms and one for encryption. It is built with its offline mode, in which
//! it waits for the server to answer again by itself; what it says of itself
//! becomes the "state" events "syncing" and "offline".
//!
//! The rooms come from the SDK's list of all rooms, filtered to those that
//! the user has joined. The SDK tells of changes to that list by position,
//! and tells of a room again whenever something notable of it changes. The
//! editor wants neither positions nor all of that: it wants a "room" event
//! when what it shows of a room is new. So after each batch of changes this
//! program looks at every room of the list, and compares what it would tell
//! of the room with what it told last ([`room_changes`]). That is simple,
//! can not go out of step, and costs little: a few reads of memory for each
//! room.

use std::collections::HashMap;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{Stream, StreamExt as _};
use matrix_sdk::Client;
use matrix_sdk_ui::eyeball_im::Vector;
use matrix_sdk_ui::room_list_service::filters::new_filter_joined;
use matrix_sdk_ui::room_list_service::{RoomListItem, RoomListService};
use matrix_sdk_ui::sync_service::{State as SyncState, SyncService};

use crate::Task;
use crate::protocol::{Output, RoomInfo, State, explain, log};

/// How many rooms the list gives at once: all of them. The SDK's list is
/// made for a view that shows a page and asks for more.
const ALL_ROOMS: usize = 1_000_000;

/// How long to wait before the sync is started again after an error that
/// the SDK does not get over by itself, at first and at most.
const RESTART_DELAY: Duration = Duration::from_secs(5);
const RESTART_DELAY_MAX: Duration = Duration::from_secs(120);

/// The running sync. It stops when this is dropped.
pub struct Syncing {
    service: Arc<SyncService>,
    _tasks: Vec<Task>,
}

impl Syncing {
    /// The service of the list of rooms, through which a room that is open
    /// asks the server for more than the list needs.
    pub fn room_list(&self) -> Arc<RoomListService> {
        self.service.room_list_service()
    }
}

/// Starts the sync of a client that is logged in. The rooms that the store
/// knows are told at once, before the server has said anything.
pub async fn start(client: &Client, out: &Output) -> Result<Syncing, String> {
    let service = SyncService::builder(client.clone())
        .with_offline_mode()
        .build()
        .await
        .map_err(|error| explain("The sync could not be set up", error))?;
    let service = Arc::new(service);

    // Messages that an earlier run did not get to send are sent now.
    client
        .send_queue()
        .respawn_tasks_for_rooms_with_unsent_requests()
        .await;

    // The watch of the state is made before the start and runs after it,
    // so that it misses no change. The first "syncing" is said here and
    // not by the watch: the watch sees only the latest state, and without
    // a connection that is "offline" before it has looked once.
    let states = service.state();
    service.start().await;
    out.state(State::Syncing, None);
    let states = Task::spawn(watch_state(
        service.clone(),
        states,
        client.clone(),
        out.clone(),
    ));
    let rooms = Task::spawn(watch_rooms(service.clone(), client.clone(), out.clone()));
    Ok(Syncing {
        service,
        _tasks: vec![states, rooms],
    })
}

/// Tells the editor whether the sync runs, and starts it again when it
/// stopped with an error.
async fn watch_state(
    service: Arc<SyncService>,
    states: impl Stream<Item = SyncState>,
    client: Client,
    out: Output,
) {
    let mut states = pin!(states);
    let mut delay = RESTART_DELAY;
    // What the editor was told last, so that it is not told twice.
    let mut syncing = true;
    while let Some(state) = states.next().await {
        match state {
            SyncState::Running => {
                if !syncing {
                    out.state(State::Syncing, None);
                }
                syncing = true;
                // A queue of messages to send stops itself when sending
                // fails. Now that the server answers, they can go.
                client.send_queue().set_enabled(true).await;
            }
            SyncState::Offline => {
                delay = RESTART_DELAY;
                syncing = false;
                out.state(State::Offline, Some("the server does not answer"));
            }
            SyncState::Error(error) => {
                log(format!("the sync stopped: {error}"));
                syncing = false;
                out.state(State::Offline, Some(&format!("the sync stopped ({error})")));
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(RESTART_DELAY_MAX);
                service.start().await;
            }
            // These are the service's states of rest, which it is in only
            // before the start and while this program stops it.
            SyncState::Idle | SyncState::Terminated => {}
        }
    }
}

/// Tells the editor of the rooms, at the start and whenever what it shows
/// of one changes.
async fn watch_rooms(service: Arc<SyncService>, client: Client, out: Output) {
    let room_list = match service.room_list_service().all_rooms().await {
        Ok(room_list) => room_list,
        Err(error) => {
            let error = explain("The list of rooms could not be read", error);
            log(&error);
            out.state(State::Error, Some(&error));
            return;
        }
    };
    let (stream, controller) = room_list.entries_with_dynamic_adapters(ALL_ROOMS);
    // The stream gives nothing before it has a filter.
    controller.set_filter(Box::new(new_filter_joined()));
    let mut stream = pin!(stream);

    let mut rooms: Vector<RoomListItem> = Vector::new();
    let mut told: HashMap<String, RoomInfo> = HashMap::new();
    while let Some(diffs) = stream.next().await {
        for diff in diffs {
            diff.apply(&mut rooms);
        }
        let mut now = Vec::with_capacity(rooms.len());
        for room in &rooms {
            if !told.contains_key(room.room_id().as_str()) {
                listen_to_latest_event(&client, room).await;
            }
            now.push(room_info(room).await);
        }
        for change in room_changes(&mut told, now) {
            match change {
                RoomChange::Room(room) => out.room(&room),
                RoomChange::Removed(id) => out.room_removed(&id),
            }
        }
    }
}

/// Asks the SDK to keep the latest message of a room, which it does only
/// for the rooms that it is asked for. The time of that message is the
/// room's time of latest activity.
async fn listen_to_latest_event(client: &Client, room: &RoomListItem) {
    let Some(latest_events) = client.latest_events().await else {
        return;
    };
    if let Err(error) = latest_events.listen_to_room(room.room_id()).await {
        log(format!("no latest message for {}: {error}", room.room_id()));
    }
}

/// What the editor is told of a room.
async fn room_info(room: &RoomListItem) -> RoomInfo {
    let name = match room.cached_display_name() {
        Some(name) => name.to_string(),
        // The name was never worked out: do that, from what the store has.
        None => room
            .display_name()
            .await
            .map(|name| name.to_string())
            .unwrap_or_default(),
    };
    // The numbers are the SDK's own count from the messages that it could
    // read, so they are right in encrypted rooms too, where the server can
    // not tell a mention from another message.
    let mut unread = room.num_unread_messages();
    if unread == 0 && room.is_marked_unread() {
        unread = 1;
    }
    RoomInfo {
        id: room.room_id().to_string(),
        name,
        unread,
        mentions: room.num_unread_mentions(),
        activity: room
            .latest_event_timestamp()
            .map_or(0, |time| time.as_secs().into()),
        direct: room.is_direct().await.unwrap_or(false),
        space: room.is_space(),
    }
}

/// A thing to tell the editor of the rooms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomChange {
    Room(RoomInfo),
    Removed(String),
}

/// What to tell the editor, given what it was told and what the rooms are
/// now: the rooms that are new or not as they were told, in the order of
/// the list, and then the rooms that are no longer there. `told` is brought
/// up to date.
pub fn room_changes(told: &mut HashMap<String, RoomInfo>, now: Vec<RoomInfo>) -> Vec<RoomChange> {
    let mut changes = Vec::new();
    let mut gone: Vec<String> = told
        .keys()
        .filter(|id| !now.iter().any(|room| &room.id == *id))
        .cloned()
        .collect();
    gone.sort();
    for room in now {
        if told.get(&room.id) != Some(&room) {
            told.insert(room.id.clone(), room.clone());
            changes.push(RoomChange::Room(room));
        }
    }
    for id in gone {
        told.remove(&id);
        changes.push(RoomChange::Removed(id));
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(id: &str, name: &str, unread: u64) -> RoomInfo {
        RoomInfo {
            id: id.to_owned(),
            name: name.to_owned(),
            unread,
            mentions: 0,
            activity: 100,
            direct: false,
            space: false,
        }
    }

    #[test]
    fn every_room_is_told_once() {
        let mut told = HashMap::new();
        let now = vec![room("!a:x", "A", 0), room("!b:x", "B", 2)];
        assert_eq!(
            room_changes(&mut told, now.clone()),
            vec![
                RoomChange::Room(now[0].clone()),
                RoomChange::Room(now[1].clone())
            ]
        );
        // Nothing changed: nothing to tell.
        assert_eq!(room_changes(&mut told, now), Vec::new());
    }

    #[test]
    fn a_room_is_told_again_when_it_changes() {
        let mut told = HashMap::new();
        room_changes(&mut told, vec![room("!a:x", "A", 0), room("!b:x", "B", 2)]);

        let renamed = room("!a:x", "Tea", 0);
        let read = room("!b:x", "B", 0);
        assert_eq!(
            room_changes(&mut told, vec![renamed.clone(), room("!b:x", "B", 2)]),
            vec![RoomChange::Room(renamed.clone())]
        );
        assert_eq!(
            room_changes(&mut told, vec![renamed.clone(), read.clone()]),
            vec![RoomChange::Room(read.clone())]
        );
        let mut later = read.clone();
        later.activity = 200;
        // The order of the list does not matter, only what is in it.
        assert_eq!(
            room_changes(&mut told, vec![later.clone(), renamed]),
            vec![RoomChange::Room(later)]
        );
    }

    #[test]
    fn a_room_that_is_left_is_removed_and_can_come_back() {
        let mut told = HashMap::new();
        room_changes(&mut told, vec![room("!a:x", "A", 0), room("!b:x", "B", 0)]);
        assert_eq!(
            room_changes(&mut told, vec![room("!b:x", "B", 0)]),
            vec![RoomChange::Removed("!a:x".to_owned())]
        );
        assert_eq!(
            room_changes(&mut told, vec![room("!b:x", "B", 0)]),
            Vec::new()
        );
        assert_eq!(
            room_changes(&mut told, vec![room("!b:x", "B", 0), room("!a:x", "A", 0)]),
            vec![RoomChange::Room(room("!a:x", "A", 0))]
        );
        // When the list is emptied, as at a reset, all are removed.
        assert_eq!(
            room_changes(&mut told, Vec::new()),
            vec![
                RoomChange::Removed("!a:x".to_owned()),
                RoomChange::Removed("!b:x".to_owned())
            ]
        );
    }
}
