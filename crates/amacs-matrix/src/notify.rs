//! The messages to tell the user of.
//!
//! Which messages those are is decided by the push rules of the account,
//! which the SDK applies to every event that the sync brings, after it has
//! decrypted it. This program adds nothing to the rules but two things that
//! they do not know. A message of the user's own is never told of. And an
//! event that is older than this run of the program is not told of either:
//! at the start, the sync brings the latest message of every room, and the
//! rules would have each of them told as if it had just come.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedTimelineEvent;
use matrix_sdk::ruma::UserId;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::events::{
    AnySyncMessageLikeEvent, AnySyncTimelineEvent, SyncMessageLikeEvent,
};
use matrix_sdk::sync::Notification;
use matrix_sdk::{Client, Room};

use crate::protocol::{Kind, Output};
use crate::timeline::message_kind_and_body;

/// How much older than the start of the program an event may be and still
/// be told of: the clocks of servers are not this machine's clock.
const SLACK: Duration = Duration::from_secs(60);

/// What is told of an event: who sent it, and what it says.
#[derive(Debug, PartialEq, Eq)]
pub struct Told {
    pub sender: String,
    pub body: String,
}

/// The words for a message in a notification: its text, or what it is.
pub fn message_words(message: &MessageType) -> String {
    let (kind, body) = message_kind_and_body(message);
    match kind {
        Kind::Image => format!("sent a picture: {body}"),
        Kind::File => format!("sent a file: {body}"),
        _ => body,
    }
}

/// What to tell of an event of a room, or nothing: for an event of the
/// user's own, for one from before `since` (milliseconds since 1970), and
/// for one that is no message.
pub fn told_of(event: &AnySyncTimelineEvent, own: &UserId, since: u64) -> Option<Told> {
    if event.sender() == own || u64::from(event.origin_server_ts().0) < since {
        return None;
    }
    let body = match event {
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::RoomMessage(
            SyncMessageLikeEvent::Original(message),
        )) => message_words(&message.content.msgtype),
        AnySyncTimelineEvent::MessageLike(AnySyncMessageLikeEvent::Sticker(
            SyncMessageLikeEvent::Original(sticker),
        )) => format!("sent a picture: {}", sticker.content.body),
        AnySyncTimelineEvent::MessageLike(
            AnySyncMessageLikeEvent::CallInvite(_) | AnySyncMessageLikeEvent::RtcNotification(_),
        ) => "started a call".to_owned(),
        // The rules name other events too, such as a change of the room's
        // name by somebody of weight. There is nothing in them to read
        // here, and what was encrypted and could not be read has no words.
        _ => return None,
    };
    Some(Told {
        sender: event.sender().to_string(),
        body,
    })
}

/// The time before which events are old, for a program that starts now.
pub fn old_before_now() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(now.saturating_sub(SLACK).as_millis()).unwrap_or(u64::MAX)
}

/// Has the client tell the editor of the messages that the rules say to
/// tell of, from now on, as "notify" events.
pub async fn start(client: &Client, out: &Output) {
    let since = old_before_now();
    let out = out.clone();
    client
        .register_notification_handler(move |notification: Notification, room: Room, _: Client| {
            let out = out.clone();
            async move {
                match notification.event {
                    RawAnySyncOrStrippedTimelineEvent::Sync(raw) => {
                        let Ok(event) = raw.deserialize() else { return };
                        let Some(told) = told_of(&event, room.own_user_id(), since) else {
                            return;
                        };
                        let name = match room.get_member_no_sync(event.sender()).await {
                            Ok(Some(member)) => member.display_name().map(ToOwned::to_owned),
                            _ => None,
                        };
                        out.notify(
                            room.room_id().as_str(),
                            Some(name.as_deref().unwrap_or(&told.sender)),
                            &told.body,
                        );
                    }
                    // An event of a room that the user is invited to. The
                    // editor shows no invitations, and the server sends
                    // them again at every start: nothing is told.
                    RawAnySyncOrStrippedTimelineEvent::Stripped(_) => {}
                }
            }
        })
        .await;
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::serde::Raw;
    use matrix_sdk::ruma::user_id;
    use serde_json::json;

    use super::*;

    fn event(
        sender: &str,
        time: u64,
        kind: &str,
        content: serde_json::Value,
    ) -> AnySyncTimelineEvent {
        let raw = json!({
            "type": kind, "event_id": "$e", "sender": sender, "origin_server_ts": time,
            "content": content,
        });
        Raw::<AnySyncTimelineEvent>::from_json_string(raw.to_string())
            .unwrap()
            .deserialize()
            .unwrap()
    }

    fn message(sender: &str, time: u64, body: &str) -> AnySyncTimelineEvent {
        event(
            sender,
            time,
            "m.room.message",
            json!({"msgtype": "m.text", "body": body}),
        )
    }

    #[test]
    fn a_message_of_another_is_told() {
        let own = user_id!("@me:example.org");
        assert_eq!(
            told_of(
                &message("@ada:example.org", 5000, "me: the tea is ready"),
                own,
                1000
            ),
            Some(Told {
                sender: "@ada:example.org".to_owned(),
                body: "me: the tea is ready".to_owned()
            })
        );
    }

    #[test]
    fn own_and_old_messages_are_not_told() {
        let own = user_id!("@me:example.org");
        assert_eq!(
            told_of(&message("@me:example.org", 5000, "hello"), own, 1000),
            None
        );
        assert_eq!(
            told_of(&message("@ada:example.org", 999, "hello"), own, 1000),
            None
        );
        assert!(told_of(&message("@ada:example.org", 1000, "hello"), own, 1000).is_some());
    }

    #[test]
    fn what_is_no_text_has_words_too() {
        let own = user_id!("@me:example.org");
        let picture = event(
            "@ada:example.org",
            5000,
            "m.room.message",
            json!({"msgtype": "m.image", "body": "kettle.jpg", "url": "mxc://example.org/abc"}),
        );
        assert_eq!(
            told_of(&picture, own, 0).unwrap().body,
            "sent a picture: kettle.jpg"
        );
        let file = event(
            "@ada:example.org",
            5000,
            "m.room.message",
            json!({"msgtype": "m.file", "body": "minutes.pdf", "url": "mxc://example.org/abc"}),
        );
        assert_eq!(
            told_of(&file, own, 0).unwrap().body,
            "sent a file: minutes.pdf"
        );
        // An event that could not be decrypted has nothing to tell.
        let encrypted = event(
            "@ada:example.org",
            5000,
            "m.room.encrypted",
            json!({
                "algorithm": "m.megolm.v1.aes-sha2", "ciphertext": "x", "sender_key": "k",
                "device_id": "D", "session_id": "s",
            }),
        );
        assert_eq!(told_of(&encrypted, own, 0), None);
        let reaction = event(
            "@ada:example.org",
            5000,
            "m.reaction",
            json!({"m.relates_to": {"rel_type": "m.annotation", "event_id": "$a", "key": "+1"}}),
        );
        assert_eq!(told_of(&reaction, own, 0), None);
    }

    #[test]
    fn what_is_old_is_measured_from_the_start() {
        let before = old_before_now();
        let now: u64 = matrix_sdk::ruma::MilliSecondsSinceUnixEpoch::now().0.into();
        assert!(before < now && now - before < 120_000);
    }
}
