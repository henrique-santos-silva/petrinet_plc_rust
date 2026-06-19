//! IO mocker view.
//!
//! Port of `IOWebMocker_static/index.html`: clickable DI LEDs that toggle the
//! emulated inputs, and DO LEDs reflecting the outputs. In the original this
//! was a separate server on port 50001; here it is a route on the same app.

use leptos::*;
use protocol::ClientMsg;

use crate::state::use_app;

fn led_class(map: &std::collections::BTreeMap<String, bool>, key: &str, clickable: bool) -> String {
    let base = if clickable { "input_led led" } else { "led" };
    match map.get(key) {
        Some(true) => format!("{base} green"),
        Some(false) => format!("{base} red"),
        None => base.to_string(),
    }
}

#[component]
pub fn IoMocker() -> impl IntoView {
    let app = use_app();
    let io = app.io_state;

    let toggle_input = {
        let app = app.clone();
        move |key: String| {
            let current = app
                .io_state
                .get_untracked()
                .digital_inputs
                .get(&key)
                .copied()
                .unwrap_or(false);
            app.send(&ClientMsg::InputUpdate {
                id: key,
                value: !current,
            });
        }
    };

    view! {
        <div id="IO_monitor" class="m-2 border justify-content-center rounded l1">
            <h2 class="text-center">"Emulador de IOs"</h2>
            <div class="m-2 p-2 border rounded l2">
                <h6 class="text-center">"Entradas"</h6>
                <div class="d-flex flex-row justify-content-around rounded">
                    {(0..8).map(|i| {
                        let key = format!("DI{i}");
                        let k_class = key.clone();
                        let k_click = key.clone();
                        let toggle = toggle_input.clone();
                        view! {
                            <div class=move || led_class(&io.get().digital_inputs, &k_class, true)
                                 id=key.clone()
                                 on:click=move |_| toggle(k_click.clone())>
                                {key}
                            </div>
                        }
                    }).collect_view()}
                </div>
            </div>
            <div class="m-2 p-2 border rounded l2">
                <h6 class="text-center">"Saídas"</h6>
                <div class="d-flex flex-row justify-content-around rounded">
                    {(0..8).map(|i| {
                        let key = format!("DO{i}");
                        let k = key.clone();
                        view! {
                            <div class=move || led_class(&io.get().digital_outputs, &k, false) id=key.clone()>
                                {key}
                            </div>
                        }
                    }).collect_view()}
                </div>
                <div class="d-flex flex-row justify-content-around rounded">
                    {(8..16).map(|i| {
                        let key = format!("DO{i}");
                        let k = key.clone();
                        view! {
                            <div class=move || led_class(&io.get().digital_outputs, &k, false) id=key.clone()>
                                {key}
                            </div>
                        }
                    }).collect_view()}
                </div>
            </div>
        </div>
    }
}
