//! Debug live view (canvas).
//!
//! Port of `petri_net_live_view.html`, `petri_net_live_view.js` and
//! `infinite_canvas.js`: renders the Petri net on a pan/zoom canvas, with live
//! token markings and transition enabling colours.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::rc::Rc;

use leptos::*;
use serde_json::Value;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, MouseEvent, WheelEvent};

use crate::state::use_app;

#[derive(Clone, Default)]
struct PlaceModel {
    id: String,
    x: f64,
    y: f64,
    marking: i64,
}

#[derive(Clone, Default)]
struct TransitionModel {
    id: String,
    x: f64,
    y: f64,
    rotation: i64,
    is_timed: bool,
    is_petri_enabled: bool,
    is_signal_enabled: bool,
}

#[derive(Clone, Default)]
struct ArcModel {
    is_inhibitor: bool,
    path: Vec<(f64, f64)>,
}

#[derive(Default)]
struct CanvasModel {
    places: Vec<PlaceModel>,
    transitions: Vec<TransitionModel>,
    arcs: Vec<ArcModel>,
    extreme: (f64, f64, f64, f64), // min_x, min_y, max_x, max_y
}

#[derive(Clone, Copy)]
struct ViewTransform {
    scale: f64,
    origin_x: f64,
    origin_y: f64,
}

struct DragState {
    dragging: bool,
    start_x: f64,
    start_y: f64,
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn build_model(json: &Value) -> CanvasModel {
    let mut model = CanvasModel {
        extreme: (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        ..Default::default()
    };

    if let Some(arcs) = json.get("arcs").and_then(|v| v.as_array()) {
        for arc in arcs {
            let path = arc
                .get("graphic_path")
                .and_then(|v| v.as_array())
                .map(|pts| {
                    pts.iter()
                        .map(|p| (num(&p["x_position"]), num(&p["y_position"])))
                        .collect()
                })
                .unwrap_or_default();
            model.arcs.push(ArcModel {
                is_inhibitor: arc.get("type").and_then(|v| v.as_str()) == Some("inhibitor"),
                path,
            });
        }
    }

    let update_extreme = |x: f64, y: f64, e: &mut (f64, f64, f64, f64)| {
        e.0 = e.0.min(x);
        e.1 = e.1.min(y);
        e.2 = e.2.max(x);
        e.3 = e.3.max(y);
    };

    if let Some(places) = json.get("places").and_then(|v| v.as_array()) {
        for place in places {
            let g = &place["graphics"];
            let x = num(&g["x_position"]);
            let y = num(&g["y_position"]);
            update_extreme(x, y, &mut model.extreme);
            model.places.push(PlaceModel {
                id: place
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                x,
                y,
                marking: place
                    .get("initial_marking")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0),
            });
        }
    }

    for group in ["instantaneous_transitions", "timed_transitions"] {
        if let Some(transitions) = json.get(group).and_then(|v| v.as_array()) {
            for t in transitions {
                let g = &t["graphics"];
                let x = num(&g["x_position"]);
                let y = num(&g["y_position"]);
                update_extreme(x, y, &mut model.extreme);
                model.transitions.push(TransitionModel {
                    id: t
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    x,
                    y,
                    rotation: g.get("rotation").and_then(|v| v.as_i64()).unwrap_or(0),
                    is_timed: t.get("timer_sec").is_some(),
                    is_petri_enabled: false,
                    is_signal_enabled: false,
                });
            }
        }
    }

    model
}

fn draw_label(ctx: &CanvasRenderingContext2d, text: &str, x: f64, y: f64) {
    ctx.set_font("9px Arial");
    let width = ctx.measure_text(text).map(|m| m.width()).unwrap_or(0.0);
    let padding = 0.2;
    let bg_w = width + padding * 2.0;
    let bg_h = 9.0 + padding * 2.0;
    let start_x = x - bg_w / 2.0;
    let start_y = y - bg_h / 2.0 + padding;
    ctx.set_fill_style_str("rgba(255, 255, 255, 0.5)");
    ctx.fill_rect(start_x, start_y, bg_w, bg_h);
    ctx.set_fill_style_str("black");
    let _ = ctx.fill_text(text, start_x + padding, y);
}

fn draw(
    ctx: &CanvasRenderingContext2d,
    canvas: &HtmlCanvasElement,
    model: &CanvasModel,
    transform: ViewTransform,
) {
    ctx.clear_rect(0.0, 0.0, canvas.width() as f64, canvas.height() as f64);
    ctx.save();
    let _ = ctx.translate(transform.origin_x, transform.origin_y);
    let _ = ctx.scale(transform.scale, transform.scale);

    // Arcs
    for arc in &model.arcs {
        if arc.path.len() < 2 {
            continue;
        }
        ctx.set_fill_style_str("black");
        ctx.set_stroke_style_str("black");
        ctx.set_line_width(1.0);
        ctx.begin_path();
        ctx.move_to(arc.path[0].0, arc.path[0].1);
        for p in &arc.path[1..] {
            ctx.line_to(p.0, p.1);
        }
        ctx.stroke();

        let last = arc.path[arc.path.len() - 1];
        let second_last = arc.path[arc.path.len() - 2];
        if arc.is_inhibitor {
            let radius = 3.0;
            let angle = (last.1 - second_last.1).atan2(last.0 - second_last.0);
            let cx = last.0 - radius * angle.cos();
            let cy = last.1 - radius * angle.sin();
            ctx.begin_path();
            let _ = ctx.arc(cx, cy, radius, 0.0, 2.0 * PI);
            ctx.set_fill_style_str("white");
            ctx.fill();
            ctx.set_stroke_style_str("black");
            ctx.stroke();
        } else {
            let head = 10.0;
            let angle = (last.1 - second_last.1).atan2(last.0 - second_last.0);
            ctx.begin_path();
            ctx.move_to(last.0, last.1);
            ctx.line_to(
                last.0 - head * (angle - PI / 6.0).cos(),
                last.1 - head * (angle - PI / 6.0).sin(),
            );
            ctx.move_to(last.0, last.1);
            ctx.line_to(
                last.0 - head * (angle + PI / 6.0).cos(),
                last.1 - head * (angle + PI / 6.0).sin(),
            );
            ctx.set_stroke_style_str("black");
            ctx.set_line_width(2.0);
            ctx.stroke();
        }
    }

    // Places
    const PLACE_R: f64 = 15.0;
    const P_X: f64 = 12.0;
    const P_Y: f64 = 12.0;
    const TOKEN_R: f64 = 3.0;
    for place in &model.places {
        let cx = place.x + P_X;
        let cy = place.y + P_Y;
        // erase + outline
        ctx.begin_path();
        let _ = ctx.arc(cx, cy, PLACE_R, 0.0, 2.0 * PI);
        ctx.set_fill_style_str("white");
        ctx.fill();
        ctx.begin_path();
        let _ = ctx.arc(cx, cy, PLACE_R, 0.0, 2.0 * PI);
        ctx.set_stroke_style_str("black");
        ctx.stroke();

        ctx.set_fill_style_str("black");
        let m = place.marking;
        if m == 1 {
            ctx.begin_path();
            let _ = ctx.arc(cx, cy, TOKEN_R, 0.0, 2.0 * PI);
            ctx.fill();
        } else if (2..=5).contains(&m) {
            for i in 0..m {
                let angle = i as f64 * (2.0 * PI / m as f64);
                let tx = cx + 0.5 * PLACE_R * angle.cos();
                let ty = cy + 0.5 * PLACE_R * angle.sin();
                ctx.begin_path();
                let _ = ctx.arc(tx, ty, TOKEN_R, 0.0, 2.0 * PI);
                ctx.fill();
            }
        } else if m > 5 {
            ctx.set_font("9px Arial");
            let _ = ctx.fill_text(&m.to_string(), cx - 4.0, cy + 3.0);
        }

        draw_label(ctx, &place.id, place.x, place.y - 10.0);
    }

    // Transitions
    const T_W: f64 = 11.0;
    const T_H: f64 = 31.0;
    const T_X: f64 = 6.0;
    const T_Y: f64 = -3.0;
    for t in &model.transitions {
        ctx.set_line_width(2.0);
        if t.is_timed {
            let color = if t.is_petri_enabled {
                if t.is_signal_enabled {
                    "yellow"
                } else {
                    "red"
                }
            } else {
                "black"
            };
            ctx.set_stroke_style_str(color);
            if t.rotation == 90 {
                ctx.stroke_rect(t.x + T_Y, t.y + T_X, T_H, T_W);
            } else {
                ctx.stroke_rect(t.x + T_X, t.y + T_Y, T_W, T_H);
            }
        } else {
            let color = if t.is_petri_enabled { "red" } else { "black" };
            ctx.set_fill_style_str(color);
            if t.rotation == 90 {
                ctx.fill_rect(t.x + T_Y, t.y + T_X, T_H, T_W);
            } else {
                ctx.fill_rect(t.x + T_X, t.y + T_Y, T_W, T_H);
            }
        }
        draw_label(ctx, &t.id, t.x, t.y - 10.0);
    }

    ctx.restore();
}

#[component]
pub fn DebugView() -> impl IntoView {
    let app = use_app();

    let canvas_ref = create_node_ref::<html::Canvas>();
    let model: Rc<RefCell<CanvasModel>> = Rc::new(RefCell::new(CanvasModel::default()));
    let transform = Rc::new(RefCell::new(ViewTransform {
        scale: 1.0,
        origin_x: 0.0,
        origin_y: 0.0,
    }));
    let drag = Rc::new(RefCell::new(DragState {
        dragging: false,
        start_x: 0.0,
        start_y: 0.0,
    }));

    canvas_ref.on_load(move |canvas_el| {
        let canvas: HtmlCanvasElement = (*canvas_el).clone();
        let window = web_sys::window().unwrap();
        let w = window.inner_width().unwrap().as_f64().unwrap();
        let h = window.inner_height().unwrap().as_f64().unwrap();
        canvas.set_width(w as u32);
        canvas.set_height(h as u32);

        let ctx: CanvasRenderingContext2d = canvas
            .get_context("2d")
            .unwrap()
            .unwrap()
            .dyn_into()
            .unwrap();

        // Closure that redraws using current model + transform.
        let redraw: Rc<dyn Fn()> = {
            let model = model.clone();
            let transform = transform.clone();
            let ctx = ctx.clone();
            let canvas = canvas.clone();
            Rc::new(move || {
                draw(&ctx, &canvas, &model.borrow(), *transform.borrow());
            })
        };

        // --- pan/zoom event listeners ---
        let mouse_coords = {
            let transform = transform.clone();
            let canvas = canvas.clone();
            move |ev: &MouseEvent| -> (f64, f64) {
                let t = *transform.borrow();
                let rect = canvas.get_bounding_client_rect();
                let mx = (ev.client_x() as f64 - t.origin_x - rect.left()) / t.scale;
                let my = (ev.client_y() as f64 - t.origin_y - rect.top()) / t.scale;
                (mx, my)
            }
        };

        // wheel zoom
        {
            let transform = transform.clone();
            let redraw = redraw.clone();
            let mouse_coords = mouse_coords.clone();
            let canvas2 = canvas.clone();
            let on_wheel = Closure::<dyn FnMut(WheelEvent)>::new(move |ev: WheelEvent| {
                ev.prevent_default();
                let (mx, my) = mouse_coords(ev.as_ref());
                let wheel = if ev.delta_y() < 0.0 { 1.0 } else { -1.0 };
                let zoom = (wheel * 0.1_f64).exp();
                let rect = canvas2.get_bounding_client_rect();
                let mut t = transform.borrow_mut();
                t.scale *= zoom;
                t.origin_x = ev.client_x() as f64 - rect.left() - mx * t.scale;
                t.origin_y = ev.client_y() as f64 - rect.top() - my * t.scale;
                drop(t);
                redraw();
            });
            canvas
                .add_event_listener_with_callback("wheel", on_wheel.as_ref().unchecked_ref())
                .unwrap();
            on_wheel.forget();
        }

        // mousedown
        {
            let drag = drag.clone();
            let mouse_coords = mouse_coords.clone();
            let on_down = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
                let (x, y) = mouse_coords(&ev);
                let mut d = drag.borrow_mut();
                d.dragging = true;
                d.start_x = x;
                d.start_y = y;
            });
            canvas
                .add_event_listener_with_callback("mousedown", on_down.as_ref().unchecked_ref())
                .unwrap();
            on_down.forget();
        }

        // mousemove
        {
            let drag = drag.clone();
            let transform = transform.clone();
            let redraw = redraw.clone();
            let canvas2 = canvas.clone();
            let on_move = Closure::<dyn FnMut(MouseEvent)>::new(move |ev: MouseEvent| {
                let d = drag.borrow();
                if !d.dragging {
                    return;
                }
                let (sx, sy) = (d.start_x, d.start_y);
                drop(d);
                let rect = canvas2.get_bounding_client_rect();
                let mut t = transform.borrow_mut();
                t.origin_x = ev.client_x() as f64 - rect.left() - sx * t.scale;
                t.origin_y = ev.client_y() as f64 - rect.top() - sy * t.scale;
                drop(t);
                redraw();
            });
            canvas
                .add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref())
                .unwrap();
            on_move.forget();
        }

        // mouseup
        {
            let drag = drag.clone();
            let on_up = Closure::<dyn FnMut(MouseEvent)>::new(move |_ev: MouseEvent| {
                drag.borrow_mut().dragging = false;
            });
            canvas
                .add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref())
                .unwrap();
            on_up.forget();
        }

        // --- reactive: rebuild model when the net json changes ---
        {
            let model = model.clone();
            let transform = transform.clone();
            let redraw = redraw.clone();
            let petrinet_json = app.petrinet_json;
            create_effect(move |_| {
                if let Some(json) = petrinet_json.get() {
                    let new_model = build_model(&json);
                    // initial scale to fit, like infinite_canvas window.onload
                    let (min_x, min_y, max_x, max_y) = new_model.extreme;
                    let mut t = transform.borrow_mut();
                    let sx = w / (max_x - min_x);
                    let sy = h / (max_y - min_y);
                    if sx.is_finite()
                        && sy.is_finite()
                        && (max_x - min_x) > 0.0
                        && (max_y - min_y) > 0.0
                    {
                        t.scale = 0.4 * sx.min(sy);
                    } else {
                        t.scale = 1.0;
                    }
                    drop(t);
                    *model.borrow_mut() = new_model;
                    redraw();
                }
            });
        }

        // --- reactive: apply live debug info (markings + enabling) ---
        {
            let model = model.clone();
            let redraw = redraw.clone();
            let debug_info = app.debug_info;
            create_effect(move |_| {
                let info = debug_info.get();
                let mut m = model.borrow_mut();
                if let Some(markings) = &info.places_marking {
                    for place in m.places.iter_mut() {
                        if let Some(v) = markings.get(&place.id) {
                            place.marking = *v;
                        }
                    }
                }
                let enabling: HashMap<_, _> = info.transitions_enabling_state.iter().collect();
                for t in m.transitions.iter_mut() {
                    if let Some(state) = enabling.get(&t.id) {
                        t.is_petri_enabled = state.is_petri_enabled;
                        t.is_signal_enabled = state.is_signal_enabled;
                    }
                }
                drop(m);
                redraw();
            });
        }

        redraw();
    });

    view! {
        <canvas id="petriNetCanvas" node_ref=canvas_ref></canvas>
    }
}
