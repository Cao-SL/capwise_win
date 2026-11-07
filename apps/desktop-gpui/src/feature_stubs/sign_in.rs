//! Stand-in for `src/sign_in.rs` when the `cap-cloud` feature is off. Keeps the
//! global entity and event types the windows subscribe to; an attempt fails at
//! once because there is no account to sign in to.

#![allow(dead_code)]

use gpui::{App, AppContext, Context, Entity, EventEmitter, Global};

use crate::main_window::TargetType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInOrigin {
    Settings,
    Editor,
    ScreenshotEditor,
    Overlay(TargetType),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInOutcome {
    SignedIn,
    Cancelled,
    TimedOut,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInEvent {
    Finished {
        origin: SignInOrigin,
        outcome: SignInOutcome,
    },
    AccountRefreshed,
}

pub struct SignIn;

struct SignInGlobal(Entity<SignIn>);

impl Global for SignInGlobal {}

impl EventEmitter<SignInEvent> for SignIn {}

impl SignIn {
    pub fn init(cx: &mut App) {
        let entity = cx.new(|_| Self);
        cx.set_global(SignInGlobal(entity));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<SignInGlobal>().0.clone()
    }

    pub fn is_pending(&self) -> bool {
        false
    }

    pub fn pending_origin(&self) -> Option<SignInOrigin> {
        None
    }

    pub fn start(&mut self, origin: SignInOrigin, cx: &mut Context<Self>) {
        cx.emit(SignInEvent::Finished {
            origin,
            outcome: SignInOutcome::Failed("Cap sign-in is not part of this build".into()),
        });
        cx.notify();
    }

    pub fn reopen(&mut self, _cx: &mut Context<Self>) {}

    pub fn cancel(&mut self, _cx: &mut Context<Self>) {}
}
