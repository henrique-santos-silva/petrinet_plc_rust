//! IHM control panel view.
//!
//! Port of `index.html` + `index.js`: file upload + validation, the IO monitor
//! (module selector + DI/DO LEDs), the user command buttons, and the
//! state-driven show/hide behaviour.

use leptos::*;
use protocol::{commands, ClientMsg};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{Event, FileReader, HtmlInputElement};

use crate::iopt::{validate_iopt, xml_to_iopt};
use crate::state::use_app;

fn led_class(map: &std::collections::BTreeMap<String, bool>, key: &str) -> String {
    match map.get(key) {
        Some(true) => "led green".to_string(),
        Some(false) => "led red".to_string(),
        None => "led".to_string(),
    }
}

fn alert(message: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.alert_with_message(message);
    }
}

#[component]
pub fn Ihm() -> impl IntoView {
    let app = use_app();

    // Local UI state.
    let pending_iopt = create_rw_signal::<Option<serde_json::Value>>(None);
    let show_config = create_rw_signal(false);

    let state = app.machine_state;

    // Derived visibility (port of index.js state handler).
    let hide_monitor_and_commands = move || {
        let s = state.get();
        s == "CheckingPetriNetFilesExistence" || s == "WaitingPetriNetFilesUpload" || s.is_empty()
    };
    let hide_new_file = move || {
        let s = state.get();
        ["Running", "Paused", "WaitingEndOfCycle", "DeadLock"].contains(&s.as_str())
    };

    // DeadLock alert (port of the `alert(...)` in index.js).
    create_effect(move |_| {
        if state.get() == "DeadLock" {
            alert("A Rede de Petri está em Deadlock! Finalize a execução e revise a RdP");
        }
    });

    // Button click -> state machine event.
    let send_event = {
        let app = app.clone();
        move |id: &str| {
            app.send(&ClientMsg::StateMachineEvent(id.to_string()));
        }
    };

    // File input change -> parse + validate.
    let on_file_change = {
        move |ev: Event| {
            let input: HtmlInputElement = ev.target().unwrap().dyn_into().unwrap();
            let files = match input.files() {
                Some(f) => f,
                None => return,
            };
            let file = match files.get(0) {
                Some(f) => f,
                None => return,
            };

            let reader = FileReader::new().unwrap();
            let reader_clone = reader.clone();
            let onload = Closure::<dyn FnMut()>::new(move || {
                let text = reader_clone
                    .result()
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                match xml_to_iopt(&text) {
                    Ok(mut iopt) => {
                        let errors = validate_iopt(&mut iopt);
                        if errors.is_empty() {
                            pending_iopt.set(Some(iopt));
                            show_config.set(true);
                        } else {
                            let pretty = serde_json::to_string_pretty(&errors).unwrap_or_default();
                            alert(&format!(
                                "ERROS Encontrados! Edite o seu xml e tente novamente!\n{pretty}"
                            ));
                            show_config.set(false);
                        }
                    }
                    Err(e) => alert(&format!("Erro ao processar o arquivo: {e}")),
                }
            });
            reader.set_onload(Some(onload.as_ref().unchecked_ref()));
            onload.forget();
            let _ = reader.read_as_text(&file);
        }
    };

    // "Enviar" the parsed IOPT config.
    let on_send_config = {
        let app = app.clone();
        move |_| {
            if let Some(iopt) = pending_iopt.get() {
                app.send(&ClientMsg::StateMachineEvent(
                    commands::FILE_UPLOADED.to_string(),
                ));
                app.send(&ClientMsg::IoptUpdate(iopt.to_string()));
            }
            pending_iopt.set(None);
            show_config.set(false);
        }
    };
    let on_cancel_config = move |_| {
        pending_iopt.set(None);
        show_config.set(false);
    };

    // IO module toggle.
    let module = app.io_module;
    let toggle_disabled = move || {
        !module.get().is_physical_io_module_enabled || state.get() != "PetriNetFilesUploaded"
    };
    let on_toggle = {
        let app = app.clone();
        move |_| {
            if module.get().is_physical_io_module {
                app.send(&ClientMsg::StateMachineEvent(
                    commands::IO_HANDLER_EMULATOR.to_string(),
                ));
            } else {
                app.send(&ClientMsg::StateMachineEvent(
                    commands::IO_HANDLER_PHYSICAL.to_string(),
                ));
            }
        }
    };

    let io = app.io_state;
    let connected = app.connected;

    // Button helpers.
    let b1 = send_event.clone();
    let b2 = send_event.clone();
    let b3 = send_event.clone();
    let b4 = send_event.clone();
    let b5 = send_event.clone();

    view! {
        <div class="container" style="max-width: 640px;">
            // ---- New file / upload ----
            <div class="m-2 border rounded l1"
                 style:display=move || if hide_new_file() { "none" } else { "block" }>
                <h2 class="text-center">"Carregue um dos arquivos da RdP"</h2>
                <div class="m-2 border d-flex flex-column justify-content-around rounded l2">
                    <div class="container my-3">
                        <label class="form-label" for="petrinet_xml_file">"Rede de Petri (XML)"</label>
                        <input type="file" class="form-control" name="file" accept=".xml"
                               id="petrinet_xml_file" on:change=on_file_change />
                    </div>
                    <a href="/api/getFile/IOPT.json" class="btn btn-secondary m-2"
                       style:display=move || if hide_monitor_and_commands() { "none" } else { "block" }>
                        "Baixar IOPT.json previamente carregado."
                    </a>
                </div>
                <div style:display=move || if show_config.get() { "block" } else { "none" }>
                    <div class="m-2 border d-flex flex-column justify-content-around rounded l2">
                        <div class="m-2 d-flex flex-row justify-content-around">
                            <button type="button" class="btn btn-secondary mx-2"
                                    on:click=on_cancel_config>"Cancelar"</button>
                            <button type="button" class="btn btn-secondary flex-grow-1 mx-2"
                                    on:click=on_send_config>"Enviar"</button>
                        </div>
                    </div>
                </div>
            </div>

            // ---- IO monitor ----
            <div class="m-2 border justify-content-center rounded l1"
                 style:display=move || if hide_monitor_and_commands() { "none" } else { "block" }>
                <h2 class="text-center">"Monitor de Sinais"</h2>
                <div class="m-2 p-2 border rounded l2">
                    <h6 class="text-center">"Seletor de Módulo de IOs"</h6>
                    <div class="d-flex flex-row justify-content-around rounded">
                        <div class="io-toggle"
                             class:on=move || module.get().is_physical_io_module
                             class:off=move || !module.get().is_physical_io_module
                             class:disabled=toggle_disabled
                             on:click=on_toggle>
                            <div class="knob">
                                {move || if module.get().is_physical_io_module {
                                    "IOs Fisícos"
                                } else {
                                    "Emulador de IOs"
                                }}
                            </div>
                        </div>
                    </div>
                </div>
                <div class="m-2 p-2 border rounded l2">
                    <h6 class="text-center">"Entradas"</h6>
                    <div class="d-flex flex-row justify-content-around rounded">
                        {(0..8).map(|i| {
                            let key = format!("DI{i}");
                            let k = key.clone();
                            view! {
                                <div class=move || led_class(&io.get().digital_inputs, &k) id=key.clone()>
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
                                <div class=move || led_class(&io.get().digital_outputs, &k) id=key.clone()>
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
                                <div class=move || led_class(&io.get().digital_outputs, &k) id=key.clone()>
                                    {key}
                                </div>
                            }
                        }).collect_view()}
                    </div>
                </div>
            </div>

            // ---- User command buttons ----
            <div class="m-2 border rounded l1"
                 style:display=move || if hide_monitor_and_commands() { "none" } else { "block" }>
                <h2 class="text-center">"Comandos do Usuário"</h2>
                <div class="m-2 d-flex flex-column justify-content-around rounded">
                    <button type="button" class="btn btn-secondary m-2"
                            prop:disabled=move || state.get() != "PetriNetFilesUploaded"
                            on:click=move |_| b1(commands::BTN_START)>"Iniciar"</button>
                    <button type="button" class="btn btn-secondary m-2"
                            prop:disabled=move || state.get() != "Running"
                            on:click=move |_| b2(commands::BTN_PAUSE)>"Interromper"</button>
                    <button type="button" class="btn btn-secondary m-2"
                            prop:disabled=move || state.get() != "Paused"
                            on:click=move |_| b3(commands::BTN_RESUME)>"Retomar"</button>
                    <button type="button" class="btn btn-secondary m-2"
                            prop:disabled=move || state.get() != "Running"
                            on:click=move |_| b4(commands::BTN_FINISH)>"Finalizar após ciclo"</button>
                    <button type="button" class="btn btn-secondary m-2"
                            prop:disabled=move || {
                                let s = state.get();
                                ["CheckingPetriNetFilesExistence", "WaitingPetriNetFilesUpload", "PetriNetFilesUploaded"]
                                    .contains(&s.as_str())
                            }
                            on:click=move |_| b5(commands::BTN_FINISH_NOW)>"Finalizar imediatamente"</button>
                </div>
            </div>

            // ---- Disconnected ----
            <div class="m-2 border rounded l1"
                 style:display=move || if connected.get() { "none" } else { "block" }>
                <h1 class="text-center">"Perda de conexão com o servidor."</h1>
                <h2 class="text-center">"Verifique se o Raspberry Pi está ligado."</h2>
            </div>
        </div>
    }
}
