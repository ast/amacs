//! Making this device one that the account trusts.
//!
//! A new device can not read the earlier messages of encrypted rooms, and
//! the others in those rooms are warned of it, until the account's own
//! identity has signed it. There are two ways to get there:
//!
//! - Another device of the account verifies this one, by comparing emojis
//!   ("verify-start", or the other device asks: "verify-request"). When
//!   both say that the emojis are the same, the other device signs this
//!   one and sends it the account's secrets, among them the key of the
//!   backup of room keys.
//! - The recovery key of the account opens the secrets that the server
//!   keeps ("recover").
//!
//! After either, the SDK has the key of the backup, and the client is built
//! so that it then fetches from the backup the key of any message that it
//! can not decrypt (see `session::build_client`). Whether the device is
//! verified goes to the editor with each "state" event, and a new one is
//! sent when that changes.
//!
//! One verification runs at a time. Only verifications between devices of
//! the user's own account are handled here: verifying another person is a
//! thing of a room and not of this device.

use std::sync::{Arc, Mutex, PoisonError};

use futures_util::StreamExt as _;
use matrix_sdk::Client;
use matrix_sdk::encryption::VerificationState;
use matrix_sdk::encryption::recovery::RecoveryError;
use matrix_sdk::encryption::secret_storage::SecretStorageError;
use matrix_sdk::encryption::verification::{
    SasState, SasVerification, Verification, VerificationRequest, VerificationRequestState,
};
use matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent;
use serde_json::Value;

use crate::Task;
use crate::protocol::{Emoji, Output, explain, log};

/// The verification that is going on.
#[derive(Default)]
struct Current {
    request: Option<VerificationRequest>,
    sas: Option<SasVerification>,
    /// The task that follows it. A new verification takes its place.
    _task: Option<Task>,
}

/// Runs the verifications of one client.
#[derive(Clone)]
pub struct Verifier {
    client: Client,
    out: Output,
    current: Arc<Mutex<Current>>,
}

/// Whether the device is verified, as the editor is told: nothing while
/// the SDK does not know yet.
pub fn verified(state: VerificationState) -> Option<bool> {
    match state {
        VerificationState::Unknown => None,
        VerificationState::Verified => Some(true),
        VerificationState::Unverified => Some(false),
    }
}

/// The emojis of a verification as the editor is told of them.
pub fn emojis<'a>(emojis: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<Emoji> {
    emojis
        .into_iter()
        .map(|(symbol, description)| Emoji {
            symbol: symbol.to_owned(),
            description: description.to_owned(),
        })
        .collect()
}

/// What to say when the recovery failed. The errors of a key that is not
/// right show parts of what was typed, so they are not passed on.
pub fn recovery_error(error: &RecoveryError) -> String {
    match error {
        RecoveryError::SecretStorage(
            SecretStorageError::SecretStorageKey(_) | SecretStorageError::MissingKeyInfo { .. },
        ) => "The recovery key is not correct.".to_owned(),
        RecoveryError::SecretStorage(SecretStorageError::ImportError { name, .. }) => {
            format!("The recovery key did not open the secret {name}.")
        }
        other => explain("The recovery failed", other),
    }
}

impl Verifier {
    /// A verifier for a client that is logged in, and the task that tells
    /// the editor whether the device is verified. Requests of other devices
    /// are heard from now on.
    pub fn start(client: &Client, out: &Output) -> (Self, Task) {
        let verifier = Self {
            client: client.clone(),
            out: out.clone(),
            current: Arc::default(),
        };

        let mut states = client.encryption().verification_state();
        out.set_verified(verified(states.get()));
        let watch = {
            let out = out.clone();
            Task::spawn(async move {
                while let Some(state) = states.next().await {
                    out.set_verified(verified(state));
                }
            })
        };

        let incoming = verifier.clone();
        client.add_event_handler(
            move |event: ToDeviceKeyVerificationRequestEvent, client: Client| {
                let verifier = incoming.clone();
                async move {
                    let request = client
                        .encryption()
                        .get_verification_request(&event.sender, &event.content.transaction_id)
                        .await;
                    if let Some(request) = request {
                        verifier.asked(request);
                    }
                }
            },
        );
        (verifier, watch)
    }

    fn current(&self) -> std::sync::MutexGuard<'_, Current> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a verification is going on.
    fn busy(&self) -> bool {
        let current = self.current();
        let request = current
            .request
            .as_ref()
            .is_some_and(|r| !r.is_done() && !r.is_cancelled());
        let sas = current
            .sas
            .as_ref()
            .is_some_and(|s| !s.is_done() && !s.is_cancelled());
        request || sas
    }

    /// Another device asks to verify this one.
    fn asked(&self, request: VerificationRequest) {
        if !request.is_self_verification() {
            log("a request to verify from another user was ignored");
            return;
        }
        if self.busy() {
            log("a request to verify was ignored: one is going on");
            return;
        }
        self.follow(request);
        self.out.verify_request();
    }

    /// Takes a request as the one that is going on, and follows it.
    fn follow(&self, request: VerificationRequest) {
        let task = Task::spawn(self.clone().follow_request(request.clone()));
        *self.current() = Current {
            request: Some(request),
            sas: None,
            _task: Some(task),
        };
    }

    /// The end of the verification that is going on.
    fn done(&self, ok: bool, message: Option<&str>) {
        {
            let mut current = self.current();
            current.request = None;
            current.sas = None;
        }
        self.out.verify_done(ok, message);
    }

    /// Follows a request until it has become a comparison of emojis, or
    /// ended.
    async fn follow_request(self, request: VerificationRequest) {
        let mut changes = request.changes();
        let mut state = request.state();
        loop {
            match state {
                // The other device is ready. The one that asked starts the
                // comparison; when the other device asked, it starts.
                VerificationRequestState::Ready { .. } if request.we_started() => {
                    match request.start_sas().await {
                        Ok(Some(sas)) => return self.follow_sas(sas).await,
                        // The other device was first: its start follows.
                        Ok(None) => {}
                        Err(error) => {
                            let error = explain("The comparison could not be started", error);
                            return self.done(false, Some(&error));
                        }
                    }
                }
                VerificationRequestState::Transitioned { verification } => {
                    return match verification {
                        Verification::SasV1(sas) => self.follow_sas(sas).await,
                        _ => {
                            let _ = request.cancel().await;
                            self.done(false, Some("the other device wants to verify in a way that this program does not have"));
                        }
                    };
                }
                VerificationRequestState::Done => return self.done(true, None),
                VerificationRequestState::Cancelled(info) => {
                    return self.done(false, Some(info.reason()));
                }
                _ => {}
            }
            match changes.next().await {
                Some(next) => state = next,
                None => return,
            }
        }
    }

    /// Follows a comparison of emojis to its end.
    async fn follow_sas(self, sas: SasVerification) {
        self.current().sas = Some(sas.clone());
        let mut changes = sas.changes();
        // When the other device started the comparison, this one agrees to
        // it. That is no answer to whether the emojis are the same.
        if !sas.we_started()
            && let Err(error) = sas.accept().await
        {
            let error = explain("The comparison could not be started", error);
            return self.done(false, Some(&error));
        }
        let mut state = sas.state();
        loop {
            match state {
                SasState::KeysExchanged {
                    emojis: Some(short),
                    ..
                } => {
                    let list = emojis(
                        short
                            .emojis
                            .iter()
                            .map(|emoji| (emoji.symbol, emoji.description)),
                    );
                    self.out.verify_emojis(&list);
                }
                SasState::KeysExchanged { emojis: None, .. } => {
                    let _ = sas.cancel().await;
                    return self.done(false, Some("the other device can not show emojis"));
                }
                SasState::Done { .. } => return self.done(true, None),
                SasState::Cancelled(info) => return self.done(false, Some(info.reason())),
                _ => {}
            }
            match changes.next().await {
                Some(next) => state = next,
                None => return,
            }
        }
    }

    /// "verify-start": asks the other devices of the account to verify
    /// this one. What follows comes as events.
    pub async fn request(&self) -> Result<Value, String> {
        if self.busy() {
            return Err("A verification is going on already.".to_owned());
        }
        let user = self
            .client
            .user_id()
            .ok_or("You are not logged in.")?
            .to_owned();
        let encryption = self.client.encryption();
        let failed =
            |error: &dyn std::fmt::Display| explain("The other devices could not be asked", error);
        let identity = match encryption
            .get_user_identity(&user)
            .await
            .map_err(|e| failed(&e))?
        {
            Some(identity) => Some(identity),
            // Not in the store yet: the server is asked.
            None => encryption
                .request_user_identity(&user)
                .await
                .map_err(|e| failed(&e))?,
        };
        let identity = identity.ok_or(
            "Your account has no identity to verify this device with. \
             Set up encryption in another Matrix program first.",
        )?;
        let request = identity
            .request_verification()
            .await
            .map_err(|e| failed(&e))?;
        self.follow(request);
        Ok(Value::Null)
    }

    /// "verify-accept": says yes to the request of another device.
    pub async fn accept(&self) -> Result<Value, String> {
        let request = self.current().request.clone();
        let request = request.ok_or("No device asks to verify this one.")?;
        request
            .accept()
            .await
            .map_err(|error| explain("The request could not be accepted", error))?;
        Ok(Value::Null)
    }

    /// "verify-cancel": ends the verification that is going on, if one is.
    /// The "verify-done" event follows when the SDK has ended it.
    pub async fn cancel(&self) -> Result<Value, String> {
        let (request, sas) = {
            let current = self.current();
            (current.request.clone(), current.sas.clone())
        };
        let failed =
            |error: matrix_sdk::Error| explain("The verification could not be ended", error);
        if let Some(sas) = sas {
            sas.cancel().await.map_err(failed)?;
        } else if let Some(request) = request {
            request.cancel().await.map_err(failed)?;
        }
        Ok(Value::Null)
    }

    /// "verify-confirm": the user says whether both devices show the same
    /// emojis. The "verify-done" event follows.
    pub async fn confirm(&self, matches: bool) -> Result<Value, String> {
        let sas = self.current().sas.clone();
        let sas = sas.ok_or("No emojis are waiting for an answer.")?;
        let outcome = if matches {
            sas.confirm().await
        } else {
            sas.mismatch().await
        };
        outcome.map_err(|error| explain("The answer could not be sent", error))?;
        Ok(Value::Null)
    }

    /// "recover": opens the account's secrets on the server with the
    /// recovery key, which makes this device a verified one and gives it
    /// the key of the backup of room keys.
    pub async fn recover(&self, key: &str) -> Result<Value, String> {
        let key = key.trim();
        if key.is_empty() {
            return Err("The recovery key is empty.".to_owned());
        }
        self.client
            .encryption()
            .recovery()
            .recover(key)
            .await
            .map_err(|error| recovery_error(&error))?;
        // The watch of the state tells the editor too, when the SDK has
        // seen what changed. This is for the editor's answer to be right
        // at once.
        self.out.set_verified(verified(
            self.client.encryption().verification_state().get(),
        ));
        Ok(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_is_told_only_what_is_known() {
        assert_eq!(verified(VerificationState::Unknown), None);
        assert_eq!(verified(VerificationState::Verified), Some(true));
        assert_eq!(verified(VerificationState::Unverified), Some(false));
    }

    #[test]
    fn emojis_have_a_symbol_and_a_description() {
        let list = emojis([("🐶", "Dog"), ("🔑", "Key")]);
        assert_eq!(
            serde_json::json!(list),
            serde_json::json!([
                {"symbol": "🐶", "description": "Dog"},
                {"symbol": "🔑", "description": "Key"},
            ])
        );
    }
}
