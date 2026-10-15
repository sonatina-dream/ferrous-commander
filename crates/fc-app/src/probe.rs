//! Layout probes for the smoke run: invisible markers that record where elements were
//! laid out, so overflow can be checked without looking at the screen.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{canvas, App, Bounds, Global, IntoElement, Pixels, Styled};

/// Recorded element bounds by name (installed only in smoke runs).
#[derive(Default)]
pub struct Probes(pub RefCell<HashMap<String, Bounds<Pixels>>>);

impl Global for Probes {}

/// An absolutely positioned, invisible child that records its parent's bounds.
pub fn probe(name: impl Into<String>) -> impl IntoElement {
    let name = name.into();
    canvas(
        move |bounds, _window, cx: &mut App| {
            if let Some(p) = cx.try_global::<Probes>() {
                p.0.borrow_mut().insert(name.clone(), bounds);
            }
        },
        |_, _, _, _| {},
    )
    // Pinned to the padding-box origin; without insets an absolute child would start
    // after the parent's padding and overshoot its right edge.
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub fn get(cx: &App, name: &str) -> Option<Bounds<Pixels>> {
    cx.try_global::<Probes>().and_then(|p| p.0.borrow().get(name).copied())
}
