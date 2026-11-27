//! The messages of the rooms that are open in the editor.
//!
//! The SDK's `Timeline` keeps the list of what happened in a room: messages
//! with their edits and reactions folded in, decrypted where the keys are
//! there, and the messages that are being sent. It tells of changes to the
//! list by position. The editor keeps its own list and knows no positions:
//! it can replace the list ("reset"), add at either end ("append",
//! "prepend"), and replace or remove items by their ids ("update",
//! "remove").
//!
//! [`Mirror`] stands between the two. It holds the list as the SDK has it,
//! with a hole for each item that the editor is not told of (the lines
//! between days, the marker of what was read), and turns each change of the
//! SDK into the editor's operations. What can not be said simply, as an
//! insertion in the middle, is said with a "reset" of the whole list:
//! correct before clever.
//!
//! # Ids, and messages that are being sent
//!
//! An item's id is the SDK's unique id of the item in this list. It is not
//! the id of the event: a message that is being sent has no event id yet.
//! When the server confirms such a message, the SDK removes the item of the
//! local message and inserts the item of the server's event *with the same
//! unique id*, where it belongs among the events of the server. The editor
//! then gets "remove" of that id and "append" of an item with that id, in
//! this order, and its list ends up right. If the place is not the end,
//! because another message is still being sent after it, the editor gets a
//! "reset". While a message is sent or fails to be sent, its item changes in
//! place, which is an "update".
//!
//! The unique ids are only good while the room is open: a room that is
//! opened again gets a "reset" with new ids.

use std::sync::Arc;

use futures_util::StreamExt as _;
use matrix_sdk::Client;
use matrix_sdk::ruma::events::room::message::{MessageType, RoomMessageEventContent};
use matrix_sdk::ruma::{OwnedRoomId, UserId};
use matrix_sdk_ui::RoomListService;
use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::timeline::{
    AnyOtherStateEventContentChange, EventSendState, EventTimelineItem, MemberProfileChange,
    MembershipChange, MsgLikeContent, MsgLikeKind, RoomExt as _, Timeline, TimelineDetails,
    TimelineItem, TimelineItemContent,
};
use serde_json::{Value, json};

use crate::Task;
use crate::protocol::{Item, Kind, Op, Output, Reaction, ReplyTo, explain, log};

/// A room that shows fewer messages than this when it is opened asks the
/// server for earlier ones at once. Without that, a room that the store
/// knows only from the list of rooms would show its last message and no
/// more.
const FILL_BELOW: usize = 20;
const FILL_COUNT: u16 = 30;

/// The most messages that one "paginate-back" may ask for.
const PAGE_MAX: u64 = 500;

/// The SDK's list of a room as far as the editor is told of it: an item, or
/// a hole where the SDK has something that the editor is not told of.
#[derive(Debug, Default)]
pub struct Mirror {
    slots: Vec<Option<Item>>,
}

impl Mirror {
    /// The items that the editor has, in order.
    pub fn items(&self) -> Vec<Item> {
        self.slots.iter().flatten().cloned().collect()
    }

    fn visible_before(&self, index: usize) -> bool {
        self.slots[..index].iter().any(Option::is_some)
    }

    fn visible_after(&self, index: usize) -> bool {
        self.slots[index + 1..].iter().any(Option::is_some)
    }

    /// Says how an item that just came to be at `index` reaches the
    /// editor: at an end of its list, or not simply.
    fn arrive(&self, index: usize, ops: &mut Vec<Op>, reset: &mut bool) {
        let Some(item) = self.slots[index].clone() else {
            return;
        };
        if !self.visible_after(index) {
            ops.push(Op::Append(vec![item]));
        } else if !self.visible_before(index) {
            ops.push(Op::Prepend(vec![item]));
        } else {
            *reset = true;
        }
    }

    /// Makes one batch of the SDK's changes to the list, and gives the
    /// operations that make the same changes to the editor's list.
    pub fn apply(&mut self, diffs: Vec<VectorDiff<Option<Item>>>) -> Vec<Op> {
        let mut ops = Vec::new();
        // Set when a change can not be said with the simple operations.
        // The rest of the batch is still made here, and the editor gets
        // the list as it is at the end.
        let mut reset = false;
        for diff in diffs {
            match diff {
                VectorDiff::Append { values } => {
                    let items: Vec<Item> = values.iter().flatten().cloned().collect();
                    self.slots.extend(values);
                    if !items.is_empty() {
                        ops.push(Op::Append(items));
                    }
                }
                VectorDiff::PushBack { value } => {
                    self.slots.push(value);
                    self.arrive(self.slots.len() - 1, &mut ops, &mut reset);
                }
                VectorDiff::PushFront { value } => {
                    self.slots.insert(0, value);
                    self.arrive(0, &mut ops, &mut reset);
                }
                VectorDiff::Insert { index, value } => {
                    if index > self.slots.len() {
                        reset = true;
                        continue;
                    }
                    self.slots.insert(index, value);
                    self.arrive(index, &mut ops, &mut reset);
                }
                VectorDiff::Set { index, value } => {
                    let Some(slot) = self.slots.get_mut(index) else {
                        reset = true;
                        continue;
                    };
                    match (std::mem::replace(slot, value.clone()), value) {
                        (None, None) => {}
                        (Some(old), None) => ops.push(Op::Remove(vec![old.id])),
                        (None, Some(_)) => self.arrive(index, &mut ops, &mut reset),
                        (Some(old), Some(new)) if old.id == new.id => {
                            // The SDK also sets an item for things that the
                            // editor is not told, such as who has read it.
                            if old != new {
                                ops.push(Op::Update(vec![new]));
                            }
                        }
                        // Another item took the place: the editor can not
                        // be told where.
                        (Some(_), Some(_)) => reset = true,
                    }
                }
                VectorDiff::Remove { index } => {
                    if index >= self.slots.len() {
                        reset = true;
                        continue;
                    }
                    if let Some(old) = self.slots.remove(index) {
                        ops.push(Op::Remove(vec![old.id]));
                    }
                }
                VectorDiff::PopBack => {
                    if let Some(Some(old)) = self.slots.pop() {
                        ops.push(Op::Remove(vec![old.id]));
                    }
                }
                VectorDiff::PopFront => {
                    if !self.slots.is_empty()
                        && let Some(old) = self.slots.remove(0)
                    {
                        ops.push(Op::Remove(vec![old.id]));
                    }
                }
                VectorDiff::Truncate { length } => {
                    if length < self.slots.len() {
                        let ids: Vec<String> = self
                            .slots
                            .drain(length..)
                            .flatten()
                            .map(|item| item.id)
                            .collect();
                        if !ids.is_empty() {
                            ops.push(Op::Remove(ids));
                        }
                    }
                }
                VectorDiff::Clear => {
                    self.slots.clear();
                    reset = true;
                }
                VectorDiff::Reset { values } => {
                    self.slots = values.into_iter().collect();
                    reset = true;
                }
            }
        }
        if reset {
            return vec![Op::Reset(self.items())];
        }
        join_ops(ops)
    }
}

/// Makes one operation of neighbours of one kind, so that the editor draws
/// less often.
fn join_ops(ops: Vec<Op>) -> Vec<Op> {
    let mut joined: Vec<Op> = Vec::with_capacity(ops.len());
    for op in ops {
        match (joined.last_mut(), op) {
            (Some(Op::Append(items)), Op::Append(more)) => items.extend(more),
            // What is put before the first later is earlier in the list.
            (Some(Op::Prepend(items)), Op::Prepend(mut more)) => {
                more.append(items);
                *items = more;
            }
            (Some(Op::Remove(ids)), Op::Remove(more)) => ids.extend(more),
            // Two updates of one item must stay in their order, which they
            // do: the editor takes the first that fits, so the later one
            // replaces the earlier one here.
            (Some(Op::Update(items)), Op::Update(more)) => {
                for item in more {
                    match items.iter_mut().find(|old| old.id == item.id) {
                        Some(old) => *old = item,
                        None => items.push(item),
                    }
                }
            }
            (_, op) => joined.push(op),
        }
    }
    joined
}

/// The kind and the text of a message, by its type.
pub fn message_kind_and_body(message: &MessageType) -> (Kind, String) {
    let body = message.body().to_owned();
    match message {
        MessageType::Text(_) => (Kind::Text, body),
        MessageType::Notice(_) | MessageType::ServerNotice(_) => (Kind::Notice, body),
        MessageType::Emote(_) => (Kind::Emote, body),
        MessageType::Image(_) => (Kind::Image, body),
        MessageType::File(_) | MessageType::Audio(_) | MessageType::Video(_) => (Kind::File, body),
        MessageType::Location(_) => (Kind::Other, format!("shared a place: {body}")),
        MessageType::VerificationRequest(_) => (Kind::Other, "asked to verify a device".to_owned()),
        // The types that come later have a text to fall back to.
        _ => (Kind::Text, body),
    }
}

/// What happened to a member of a room, in a few words that follow the name
/// of who did it. `other` is the member, when that is not who did it.
pub fn membership_body(change: Option<MembershipChange>, other: Option<&str>) -> String {
    use MembershipChange as M;
    let (text, names_other) = match change {
        Some(M::Joined) => ("joined", false),
        Some(M::Left) => ("left", false),
        Some(M::Banned) => ("banned", true),
        Some(M::Unbanned) => ("unbanned", true),
        Some(M::Kicked) => ("removed", true),
        Some(M::Invited) => ("invited", true),
        Some(M::KickedAndBanned) => ("removed and banned", true),
        Some(M::InvitationAccepted) => ("accepted the invitation", false),
        Some(M::InvitationRejected) => ("rejected the invitation", false),
        Some(M::InvitationRevoked) => ("took back the invitation of", true),
        Some(M::Knocked) => ("asked to join", false),
        Some(M::KnockAccepted) => ("let in", true),
        Some(M::KnockRetracted) => ("no longer asks to join", false),
        Some(M::KnockDenied) => ("refused to let in", true),
        Some(M::None | M::Error | M::NotImplemented) | None => ("changed the membership", false),
    };
    match other {
        Some(other) if names_other => format!("{text} {other}"),
        _ => text.to_owned(),
    }
}

/// What a member changed of their profile.
fn profile_body(change: &MemberProfileChange) -> String {
    let name = change
        .displayname_change()
        .and_then(|change| change.new.clone());
    match (
        change.displayname_change().is_some(),
        change.avatar_url_change().is_some(),
    ) {
        (true, false) => match name {
            Some(name) => format!("changed their name to {name}"),
            None => "removed their name".to_owned(),
        },
        (false, true) => "changed their picture".to_owned(),
        (true, true) => "changed their name and their picture".to_owned(),
        (false, false) => "changed their profile".to_owned(),
    }
}

/// What a change to the state of a room did, in a few words.
fn state_body(change: &AnyOtherStateEventContentChange) -> String {
    use AnyOtherStateEventContentChange as S;
    match change {
        S::RoomName(_) => "changed the room name".to_owned(),
        S::RoomTopic(_) => "changed the topic".to_owned(),
        S::RoomAvatar(_) => "changed the picture of the room".to_owned(),
        S::RoomCreate(_) => "made the room".to_owned(),
        S::RoomEncryption(_) => "turned on encryption".to_owned(),
        S::RoomCanonicalAlias(_) => "changed the address of the room".to_owned(),
        S::RoomJoinRules(_) => "changed who can join".to_owned(),
        S::RoomHistoryVisibility(_) => "changed who can read the history".to_owned(),
        S::RoomGuestAccess(_) => "changed whether guests can join".to_owned(),
        S::RoomPowerLevels(_) => "changed the permissions".to_owned(),
        S::RoomPinnedEvents(_) => "changed the pinned messages".to_owned(),
        S::RoomTombstone(_) => "replaced the room with a new one".to_owned(),
        S::RoomServerAcl(_) => "changed which servers can take part".to_owned(),
        S::SpaceChild(_) => "changed the rooms of the space".to_owned(),
        S::SpaceParent(_) => "changed the space of the room".to_owned(),
        other => format!("changed the room ({})", other.event_type()),
    }
}

/// The kind, the text and whether it was edited, of what an item holds.
/// `sender` is who sent it.
pub fn content_parts(content: &TimelineItemContent, sender: &UserId) -> (Kind, String, bool) {
    match content {
        TimelineItemContent::MsgLike(MsgLikeContent { kind, .. }) => match kind {
            MsgLikeKind::Message(message) => {
                let (kind, body) = message_kind_and_body(message.msgtype());
                (kind, body, message.is_edited())
            }
            MsgLikeKind::Sticker(sticker) => (Kind::Image, sticker.content().body.clone(), false),
            MsgLikeKind::Poll(_) => (Kind::Other, "started a poll".to_owned(), false),
            MsgLikeKind::Redacted => (Kind::Redacted, String::new(), false),
            MsgLikeKind::UnableToDecrypt(_) => (Kind::Undecryptable, String::new(), false),
            MsgLikeKind::LiveLocation(_) => {
                (Kind::Other, "shares where they are".to_owned(), false)
            }
            MsgLikeKind::Other(other) => (
                Kind::Other,
                format!("sent something else ({})", other.event_type()),
                false,
            ),
        },
        TimelineItemContent::MembershipChange(change) => {
            let other = (change.user_id() != sender).then(|| change.user_id().as_str());
            (
                Kind::Membership,
                membership_body(change.change(), other),
                false,
            )
        }
        TimelineItemContent::ProfileChange(change) => {
            (Kind::Membership, profile_body(change), false)
        }
        TimelineItemContent::OtherState(state) => (Kind::Other, state_body(state.content()), false),
        TimelineItemContent::FailedToParseMessageLike { event_type, .. } => (
            Kind::Other,
            format!("sent something that can not be read ({event_type})"),
            false,
        ),
        TimelineItemContent::FailedToParseState { event_type, .. } => (
            Kind::Other,
            format!("changed the room in a way that can not be read ({event_type})"),
            false,
        ),
        TimelineItemContent::CallInvite | TimelineItemContent::RtcNotification { .. } => {
            (Kind::Other, "started a call".to_owned(), false)
        }
    }
}

/// The message that an item answers, as far as it is known.
fn reply_to(content: &TimelineItemContent) -> Option<ReplyTo> {
    let details = content.in_reply_to()?;
    let event_id = details.event_id.to_string();
    Some(match &details.event {
        TimelineDetails::Ready(event) => ReplyTo {
            event_id,
            sender: Some(event.sender.to_string()),
            sender_name: display_name(&event.sender_profile),
            body: Some(content_parts(&event.content, &event.sender).1),
        },
        _ => ReplyTo {
            event_id,
            sender: None,
            sender_name: None,
            body: None,
        },
    })
}

fn display_name(profile: &TimelineDetails<matrix_sdk_ui::timeline::Profile>) -> Option<String> {
    match profile {
        TimelineDetails::Ready(profile) => profile.display_name.clone(),
        _ => None,
    }
}

/// What the editor is told of an event of the SDK's list. `id` is the
/// unique id of the item in the list, and `own` is the user of this
/// program.
fn event_item(id: &str, event: &EventTimelineItem, own: &UserId) -> Item {
    let content = event.content();
    let (kind, body, edited) = content_parts(content, event.sender());
    let reactions = event
        .reactions()
        .iter()
        .map(|(key, senders)| Reaction {
            key: key.clone(),
            count: senders.len() as u64,
            mine: senders.contains_key(own),
        })
        .collect();
    let send_state = match event.send_state() {
        Some(EventSendState::NotSentYet { .. }) => Some("sending"),
        Some(EventSendState::SendingFailed { .. }) => Some("failed"),
        Some(EventSendState::Sent { .. }) | None => None,
    };
    Item {
        id: id.to_owned(),
        sender: event.sender().to_string(),
        sender_name: display_name(event.sender_profile()),
        time: event.timestamp().as_secs().into(),
        kind,
        body,
        edited,
        event_id: event.event_id().map(ToString::to_string),
        send_state,
        reply_to: reply_to(content),
        reactions,
        thread_root: content.thread_root().map(|id| id.to_string()),
    }
}

/// What the editor is told of an item of the SDK's list: nothing, for the
/// items that are no events.
fn item(item: &TimelineItem, own: &UserId) -> Option<Item> {
    item.as_event()
        .map(|event| event_item(&item.unique_id().0, event, own))
}

/// A room that is open: its list of the SDK, and the task that tells the
/// editor of changes. The telling stops when this is dropped.
pub struct OpenTimeline {
    timeline: Arc<Timeline>,
    _tasks: Vec<Task>,
}

impl OpenTimeline {
    pub fn timeline(&self) -> Arc<Timeline> {
        self.timeline.clone()
    }
}

/// Opens a room: the editor gets a "reset" with the messages that are known
/// now, and then every change.
pub async fn open(
    client: &Client,
    room_list: &RoomListService,
    room_id: &OwnedRoomId,
    out: &Output,
) -> Result<OpenTimeline, String> {
    let room = client.get_room(room_id).ok_or("This room is not known.")?;
    let own = room.own_user_id().to_owned();
    let timeline = room
        .timeline()
        .await
        .map_err(|error| explain("The messages of the room could not be read", error))?;
    let timeline = Arc::new(timeline);

    // The list of rooms asks the server for little of each room. A room
    // that is open wants all that happens in it.
    room_list.set_room_subscriptions(&[room_id.as_ref()]).await;

    let (items, mut stream) = timeline.subscribe().await;
    let mut mirror = Mirror::default();
    let initial = VectorDiff::Reset {
        values: items.iter().map(|i| item(i, &own)).collect(),
    };
    let name = room_id.to_string();
    for op in mirror.apply(vec![initial]) {
        out.timeline(&name, &op);
    }
    let shown = mirror.items().len();

    let watch = {
        let out = out.clone();
        Task::spawn(async move {
            while let Some(diffs) = stream.next().await {
                let diffs = diffs
                    .into_iter()
                    .map(|diff| diff.map(|i| item(&i, &own)))
                    .collect();
                for op in mirror.apply(diffs) {
                    out.timeline(&name, &op);
                }
            }
        })
    };
    let mut tasks = vec![watch];

    if shown < FILL_BELOW {
        let timeline = timeline.clone();
        tasks.push(Task::spawn(async move {
            // If this fails, as without a connection, the room shows what
            // it has and the user can ask for earlier messages later.
            if let Err(error) = timeline.paginate_backwards(FILL_COUNT).await {
                log(format!(
                    "no earlier messages for a room that was opened: {error}"
                ));
            }
        }));
    }
    Ok(OpenTimeline {
        timeline,
        _tasks: tasks,
    })
}

/// Closes a room: the server is no longer asked for all that happens in
/// it. The caller drops the [`OpenTimeline`].
pub fn close(room_list: &RoomListService, room_id: &OwnedRoomId) {
    room_list.remove_room_subscriptions(&[room_id.as_ref()]);
}

/// Loads earlier messages, which reach the editor as changes to the list.
/// The result is false when the start of the room was reached, so that
/// there are no earlier ones.
pub async fn paginate_back(timeline: &Timeline, count: Option<u64>) -> Result<Value, String> {
    let count = count.unwrap_or(u64::from(FILL_COUNT)).clamp(1, PAGE_MAX);
    let count = u16::try_from(count).unwrap_or(FILL_COUNT);
    let reached_start = timeline
        .paginate_backwards(count)
        .await
        .map_err(|error| explain("Earlier messages could not be loaded", error))?;
    Ok(json!(!reached_start))
}

/// Sends a message of plain text. In a room that is open it goes through
/// the room's list, and so is in the list at once; in another room it goes
/// to the same queue of the SDK directly. Either way the SDK encrypts it if
/// the room is encrypted, and tries again when the server did not answer.
pub async fn send(
    client: &Client,
    open: Option<Arc<Timeline>>,
    room_id: &OwnedRoomId,
    body: &str,
) -> Result<Value, String> {
    let content = RoomMessageEventContent::text_plain(body);
    let failed = |error: &dyn std::fmt::Display| explain("The message could not be sent", error);
    match open {
        Some(timeline) => {
            timeline
                .send(content.into())
                .await
                .map_err(|error| failed(&error))?;
        }
        None => {
            let room = client.get_room(room_id).ok_or("This room is not known.")?;
            room.send_queue()
                .send(content.into())
                .await
                .map_err(|error| failed(&error))?;
        }
    }
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::events::room::message::{
        EmoteMessageEventContent, NoticeMessageEventContent,
    };
    use matrix_sdk::ruma::user_id;
    use matrix_sdk_ui::eyeball_im::Vector;

    use super::*;
    use crate::protocol::tests::item as an_item;

    /// A mirror and the editor's list beside it, which the tests keep as
    /// the editor does (`%matrix-timeline-event` in matrix.scm).
    #[derive(Default)]
    struct Pair {
        mirror: Mirror,
        editor: Vec<Item>,
    }

    impl Pair {
        fn apply(&mut self, diffs: Vec<VectorDiff<Option<Item>>>) -> Vec<Op> {
            let ops = self.mirror.apply(diffs);
            for op in &ops {
                match op.clone() {
                    Op::Reset(items) => self.editor = items,
                    Op::Append(items) => self.editor.extend(items),
                    Op::Prepend(mut items) => {
                        items.append(&mut self.editor);
                        self.editor = items;
                    }
                    Op::Update(items) => {
                        for old in &mut self.editor {
                            if let Some(new) = items.iter().find(|new| new.id == old.id) {
                                *old = new.clone();
                            }
                        }
                    }
                    Op::Remove(ids) => self.editor.retain(|item| !ids.contains(&item.id)),
                }
            }
            // Whatever was said, the editor has what the SDK has.
            assert_eq!(self.editor, self.mirror.items());
            ops
        }

        fn bodies(&self) -> Vec<&str> {
            self.editor.iter().map(|item| item.body.as_str()).collect()
        }
    }

    fn some(id: &str, body: &str) -> Option<Item> {
        Some(an_item(id, body))
    }

    fn reset(values: Vec<Option<Item>>) -> VectorDiff<Option<Item>> {
        VectorDiff::Reset {
            values: values.into_iter().collect(),
        }
    }

    /// A list as the SDK has it for a room with three messages: a line for
    /// the day first.
    fn three() -> Pair {
        let mut pair = Pair::default();
        let ops = pair.apply(vec![reset(vec![
            None,
            some("1", "a"),
            some("2", "b"),
            some("3", "c"),
        ])]);
        assert!(matches!(ops.as_slice(), [Op::Reset(items)] if items.len() == 3));
        pair
    }

    #[test]
    fn new_messages_are_appended() {
        let mut pair = three();
        let ops = pair.apply(vec![VectorDiff::PushBack {
            value: some("4", "d"),
        }]);
        assert_eq!(ops, vec![Op::Append(vec![an_item("4", "d")])]);
        // A line for a new day and two messages, in one batch.
        let ops = pair.apply(vec![
            VectorDiff::PushBack { value: None },
            VectorDiff::PushBack {
                value: some("5", "e"),
            },
            VectorDiff::Append {
                values: Vector::from(vec![some("6", "f"), None]),
            },
        ]);
        assert_eq!(
            ops,
            vec![Op::Append(vec![an_item("5", "e"), an_item("6", "f")])]
        );
        assert_eq!(pair.bodies(), ["a", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn earlier_messages_are_prepended() {
        let mut pair = three();
        // The SDK puts earlier messages after the line of the day, one by
        // one, each before the one that it put last.
        let ops = pair.apply(vec![
            VectorDiff::Insert {
                index: 1,
                value: some("0", "z"),
            },
            VectorDiff::Insert {
                index: 1,
                value: some("-1", "y"),
            },
            VectorDiff::PushFront { value: None },
        ]);
        assert_eq!(
            ops,
            vec![Op::Prepend(vec![an_item("-1", "y"), an_item("0", "z")])]
        );
        assert_eq!(pair.bodies(), ["y", "z", "a", "b", "c"]);
        let ops = pair.apply(vec![VectorDiff::PushFront {
            value: some("-2", "x"),
        }]);
        assert_eq!(ops, vec![Op::Prepend(vec![an_item("-2", "x")])]);
    }

    #[test]
    fn an_insertion_in_the_middle_is_a_reset() {
        let mut pair = three();
        let ops = pair.apply(vec![
            VectorDiff::PushBack {
                value: some("4", "d"),
            },
            VectorDiff::Insert {
                index: 2,
                value: some("9", "between"),
            },
        ]);
        assert!(matches!(ops.as_slice(), [Op::Reset(items)] if items.len() == 5));
        assert_eq!(pair.bodies(), ["a", "between", "b", "c", "d"]);
    }

    #[test]
    fn a_changed_message_is_an_update() {
        let mut pair = three();
        let mut edited = an_item("2", "b, with more");
        edited.edited = true;
        let ops = pair.apply(vec![VectorDiff::Set {
            index: 2,
            value: Some(edited.clone()),
        }]);
        assert_eq!(ops, vec![Op::Update(vec![edited.clone()])]);
        // A set that changes nothing that the editor is told says nothing.
        assert_eq!(
            pair.apply(vec![VectorDiff::Set {
                index: 2,
                value: Some(edited)
            }]),
            Vec::new()
        );
        // Two changes of one item in a batch: the last one counts.
        let ops = pair.apply(vec![
            VectorDiff::Set {
                index: 1,
                value: some("1", "a2"),
            },
            VectorDiff::Set {
                index: 1,
                value: some("1", "a3"),
            },
            VectorDiff::Set {
                index: 3,
                value: some("3", "c2"),
            },
        ]);
        assert_eq!(
            ops,
            vec![Op::Update(vec![an_item("1", "a3"), an_item("3", "c2")])]
        );
        assert_eq!(pair.bodies(), ["a3", "b, with more", "c2"]);
    }

    #[test]
    fn a_set_with_another_id_is_a_reset() {
        let mut pair = three();
        let ops = pair.apply(vec![VectorDiff::Set {
            index: 2,
            value: some("8", "other"),
        }]);
        assert!(matches!(ops.as_slice(), [Op::Reset(_)]));
        assert_eq!(pair.bodies(), ["a", "other", "c"]);
    }

    #[test]
    fn removals_name_the_ids() {
        let mut pair = three();
        let ops = pair.apply(vec![VectorDiff::Remove { index: 2 }, VectorDiff::PopBack]);
        assert_eq!(ops, vec![Op::Remove(vec!["2".to_owned(), "3".to_owned()])]);
        // The line of the day goes without a word, and then the message.
        assert_eq!(pair.apply(vec![VectorDiff::PopFront]), Vec::new());
        assert_eq!(
            pair.apply(vec![VectorDiff::PopFront]),
            vec![Op::Remove(vec!["1".to_owned()])]
        );
        assert_eq!(
            pair.apply(vec![VectorDiff::PopFront, VectorDiff::PopBack]),
            Vec::new()
        );

        let mut pair = three();
        let ops = pair.apply(vec![VectorDiff::Truncate { length: 2 }]);
        assert_eq!(ops, vec![Op::Remove(vec!["2".to_owned(), "3".to_owned()])]);
        assert_eq!(
            pair.apply(vec![VectorDiff::Clear]),
            vec![Op::Reset(Vec::new())]
        );
    }

    #[test]
    fn what_is_no_event_comes_and_goes_without_a_word() {
        let mut pair = three();
        let ops = pair.apply(vec![
            VectorDiff::Insert {
                index: 2,
                value: None,
            },
            VectorDiff::Set {
                index: 2,
                value: None,
            },
            VectorDiff::Remove { index: 2 },
            VectorDiff::PushBack { value: None },
        ]);
        assert_eq!(ops, Vec::new());
        // An event where a marker was is an arrival, here at the end.
        let ops = pair.apply(vec![VectorDiff::Set {
            index: 4,
            value: some("4", "d"),
        }]);
        assert_eq!(ops, vec![Op::Append(vec![an_item("4", "d")])]);
        // And a marker where an event was is a removal.
        let ops = pair.apply(vec![VectorDiff::Set {
            index: 4,
            value: None,
        }]);
        assert_eq!(ops, vec![Op::Remove(vec!["4".to_owned()])]);
    }

    #[test]
    fn a_sent_message_takes_the_place_of_its_echo() {
        let mut pair = three();
        // The message that is being sent comes at the end.
        let mut echo = an_item("4", "on my way");
        echo.send_state = Some("sending");
        let ops = pair.apply(vec![VectorDiff::PushBack {
            value: Some(echo.clone()),
        }]);
        assert_eq!(ops, vec![Op::Append(vec![echo])]);

        // The server has it: the SDK removes the echo and inserts the
        // event, with the id of the echo, after the last event of the
        // server.
        let mut sent = an_item("4", "on my way");
        sent.event_id = Some("$event".to_owned());
        let ops = pair.apply(vec![
            VectorDiff::Remove { index: 4 },
            VectorDiff::Insert {
                index: 4,
                value: Some(sent.clone()),
            },
        ]);
        assert_eq!(
            ops,
            vec![
                Op::Remove(vec!["4".to_owned()]),
                Op::Append(vec![sent.clone()])
            ]
        );
        assert_eq!(pair.editor.last(), Some(&sent));
        assert_eq!(pair.editor.len(), 4);
    }

    #[test]
    fn a_sent_message_before_another_echo_is_a_reset() {
        let mut pair = three();
        pair.apply(vec![
            VectorDiff::PushBack {
                value: some("4", "first"),
            },
            VectorDiff::PushBack {
                value: some("5", "second"),
            },
        ]);
        // The first is confirmed while the second is still on its way.
        let mut sent = an_item("4", "first");
        sent.event_id = Some("$first".to_owned());
        let ops = pair.apply(vec![
            VectorDiff::Remove { index: 4 },
            VectorDiff::Insert {
                index: 4,
                value: Some(sent),
            },
        ]);
        assert!(matches!(ops.as_slice(), [Op::Reset(items)] if items.len() == 5));
        assert_eq!(pair.bodies(), ["a", "b", "c", "first", "second"]);
    }

    #[test]
    fn positions_that_can_not_be_are_a_reset_and_no_panic() {
        let mut pair = three();
        let ops = pair.apply(vec![
            VectorDiff::Remove { index: 40 },
            VectorDiff::Insert {
                index: 40,
                value: some("x", "x"),
            },
            VectorDiff::Set {
                index: 40,
                value: some("y", "y"),
            },
        ]);
        assert!(matches!(ops.as_slice(), [Op::Reset(items)] if items.len() == 3));
    }

    #[test]
    fn messages_have_the_kind_of_their_type() {
        let kind = |message: MessageType| message_kind_and_body(&message);
        assert_eq!(
            kind(MessageType::text_plain("hello")),
            (Kind::Text, "hello".to_owned())
        );
        assert_eq!(
            kind(MessageType::Notice(NoticeMessageEventContent::plain(
                "a notice"
            ))),
            (Kind::Notice, "a notice".to_owned())
        );
        assert_eq!(
            kind(MessageType::Emote(EmoteMessageEventContent::plain("waves"))),
            (Kind::Emote, "waves".to_owned())
        );
    }

    #[test]
    fn what_is_no_message_has_its_kind_too() {
        let sender = user_id!("@ada:example.org");
        let redacted = TimelineItemContent::MsgLike(MsgLikeContent::redacted());
        assert_eq!(
            content_parts(&redacted, sender),
            (Kind::Redacted, String::new(), false)
        );
        assert_eq!(
            content_parts(&TimelineItemContent::CallInvite, sender),
            (Kind::Other, "started a call".to_owned(), false)
        );
    }

    #[test]
    fn membership_is_said_in_a_few_words() {
        use MembershipChange as M;
        assert_eq!(membership_body(Some(M::Joined), None), "joined");
        assert_eq!(membership_body(Some(M::Left), None), "left");
        assert_eq!(
            membership_body(Some(M::Invited), Some("@bo:x")),
            "invited @bo:x"
        );
        assert_eq!(
            membership_body(Some(M::Kicked), Some("@bo:x")),
            "removed @bo:x"
        );
        assert_eq!(
            membership_body(Some(M::Banned), Some("@bo:x")),
            "banned @bo:x"
        );
        // Who joined is who sent the event, also when the SDK names them.
        assert_eq!(membership_body(Some(M::Joined), Some("@bo:x")), "joined");
        assert_eq!(membership_body(Some(M::Invited), None), "invited");
        assert_eq!(membership_body(None, None), "changed the membership");
    }
}
