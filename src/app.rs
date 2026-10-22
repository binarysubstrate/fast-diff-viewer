use std::ops::Range;
use std::sync::Arc;

use eframe::egui;
use egui::{Color32, FontId, TextEdit, TextStyle, epaint::Galley, text::LayoutJob};

#[cfg(feature = "bench")]
use crate::bench;
use crate::buffer_stamp::BufferStamp;
use crate::diff::Role;
use crate::pipeline::DiffPipeline;
use crate::render::{
    LayoutPaint, PaintIdentity, PaneSide, build_layout_job, gutter_marker_color,
    gutter_marker_for_role, hash_inline_emphasis, hash_paint_inputs, hash_roles, hash_text,
    is_styled_paint_valid,
};
use crate::theme::AppTheme;

/// Memoized layout result for a pane.
///
/// Keyed by
/// `(paint_identity, roles_hash, inline_emphasis_hash, paint_hash, wrap_width_bits)`.
/// The identity covers both the current buffer stamp and the stamp that
/// produced the applied roles/emphasis; `paint_hash` captures theme-relevant
/// format colors, and `wrap_width` comes from egui's layouter input and
/// changes the resulting `Galley` even when the text is unchanged. A miss
/// rebuilds the `LayoutJob` and re-runs font layout; hits submit the cached
/// job to egui's memoized font layout so its paragraph cache stays alive.
/// `is_styled` records whether the cached paint took the styled path (vs. the
/// neutral fallback); on a cache hit the layouter re-reports the same value so
/// the bench harness's `painted_styled_this_frame` flag is correct when cached
/// paint is reused.
type LayoutCacheKey = (PaintIdentity, u64, u64, u64, u32);

struct LayoutCache {
    key: LayoutCacheKey,
    galley: Arc<Galley>,
    is_styled: bool,
}

fn layout_cache_key(
    identity: PaintIdentity,
    roles_hash: u64,
    emphasis_hash: u64,
    paint_hash: u64,
    wrap_width_bits: u32,
) -> LayoutCacheKey {
    (
        identity,
        roles_hash,
        emphasis_hash,
        paint_hash,
        wrap_width_bits,
    )
}

#[derive(Default)]
struct Pane {
    buffer: String,
    line_roles: Vec<Role>,
    inline_emphasis: Vec<Vec<Range<usize>>>,
    layout_cache: Option<LayoutCache>,
    /// Revision whose roles+emphasis currently populate this pane,
    /// advanced by `apply_diff_results`. Bench-relevant but cheap
    /// enough to keep always-on.
    applied_revision: u64,
    /// Current pane-buffer identity, advanced to the latest document
    /// revision whenever either pane dispatches a diff.
    current_stamp: BufferStamp,
    /// Identity of the pane buffer that produced the currently-applied
    /// roles and inline emphasis.
    applied_stamp: BufferStamp,
    /// True if the layouter took the styled (aligned) path during the
    /// most recent frame; false if it took the neutral fallback. Reset
    /// to false at the top of each `ui()`.
    painted_styled_this_frame: bool,
}

pub(crate) struct App {
    theme: AppTheme,
    left: Pane,
    right: Pane,
    pipeline: DiffPipeline,
    prev_left: String,
    prev_right: String,
    /// Shared scroll offset mirrored between both panes' `ScrollArea`s.
    /// Read via `ScrollAreaOutput.state.clamped_offset()` on each pane (ordinary
    /// output data, not event interception) and passed back to the
    /// opposite pane through `ScrollArea::scroll_offset(...)` the next
    /// frame.
    ///
    /// Update this field
    /// only when a pane's captured offset differs from the stored
    /// value, and only from one pane per frame — writing both outputs
    /// unconditionally every frame produces a two-frame ping-pong.
    sync_scroll_offset: egui::Vec2,
    /// Previous frame's left-pane max scroll extent. Used to clamp the
    /// next frame's shared sync input before `ScrollArea::begin()`
    /// consumes it so the shorter pane never receives an out-of-range
    /// offset.
    left_max_offset: egui::Vec2,
    /// Previous frame's right-pane max scroll extent. Mirrors
    /// `left_max_offset` for the opposite pane.
    right_max_offset: egui::Vec2,
    /// Revision that has been painted styled on both panes. Probes in
    /// bench mode resolve when `probe.revision <= this`. Monotonic.
    last_painted_styled_revision: u64,
    /// Right-pane portion of a diff result whose application has been
    /// deferred by one frame. Populated by `apply_diff_results` when a
    /// result would otherwise force both panes' layouters to rebuild
    /// the galley in the same frame on a large (>200KB) buffer — the
    /// large_paste scenario where two back-to-back 10k-section galley
    /// rebuilds in one `ui()` blow past the 100 ms per-frame budget.
    /// Consumed at the top of the next `apply_diff_results`.
    pending_right_apply: Option<PendingRightApply>,
    #[cfg(feature = "bench")]
    bench: Option<bench::BenchController>,
}

struct PendingRightApply {
    roles: Vec<Role>,
    inline_emphasis: Vec<Vec<Range<usize>>>,
    revision: u64,
    stamp: BufferStamp,
}

/// Apply the active theme to egui's visuals.
///
/// Sets panel/window background to `bg_app`, widget fills to `bg_pane`,
/// text to `ink`, the caret + selection to the Paper accents, and zeroes
/// the stroke surfaces used by `TextEdit` (inactive/hovered/active
/// `bg_stroke` plus `selection.stroke`) so no thin outline leaks onto
/// the `TextEdit` panes.
///
/// `all_styles_mut` is used instead of `style_mut` so the theme applies
/// to both `Theme::Dark` and `Theme::Light`. eframe follows the OS theme
/// by default and may switch at first frame, so a `style_mut` call that
/// only touches the current theme leaves the other theme's strokes
/// painting through and its backgrounds defaulting to egui's defaults.
fn apply_theme(ctx: &egui::Context, theme: &AppTheme) {
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.panel_fill = theme.bg_app;
        v.window_fill = theme.bg_app;
        v.extreme_bg_color = theme.bg_pane;
        // TextEdit's frame fill must be transparent so the
        // crosshatch painted behind `scroll_rect` in `render_pane`
        // shows through the editable surface. Other `extreme_bg_color`
        // consumers (unused in this app today) keep the opaque value.
        // `text_edit_bg_color` defaults to `extreme_bg_color` when
        // None; setting it explicitly overrides only the TextEdit fill.
        v.text_edit_bg_color = Some(egui::Color32::TRANSPARENT);
        v.override_text_color = Some(theme.ink);
        v.text_cursor.stroke.color = theme.caret;
        v.selection.bg_fill = theme.selection;
        // egui's `paint_text_selection` uses `selection.stroke.color`
        // for the selected glyphs. Setting this to
        // `Stroke::NONE` would suppress the focus outline but make
        // selected text invisible. Keep width 0 so no focus
        // outline draws, but paint selected glyphs in the theme's ink
        // so highlighted words stay readable.
        v.selection.stroke = egui::Stroke::new(0.0, theme.ink);
        v.widgets.noninteractive.bg_stroke.color = theme.rule;
        v.widgets.inactive.bg_stroke = egui::Stroke::NONE;
        v.widgets.hovered.bg_stroke = egui::Stroke::NONE;
        v.widgets.active.bg_stroke = egui::Stroke::NONE;
    });
}

impl App {
    #[cfg(feature = "bench")]
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        bench_config: Option<bench::BenchConfig>,
    ) -> Self {
        let bench_ctrl = bench_config.and_then(|cfg| match bench::BenchController::new(cfg) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("bench: failed to initialize controller: {e}");
                None
            }
        });
        // In bench mode, request continuous repaints so the state
        // machine advances even without user input.
        if bench_ctrl.is_some() {
            cc.egui_ctx.request_repaint();
        }
        let theme = AppTheme::graphite();
        apply_theme(&cc.egui_ctx, &theme);
        Self {
            theme,
            left: Pane::default(),
            right: Pane::default(),
            pipeline: DiffPipeline::spawn(cc.egui_ctx.clone()),
            prev_left: String::new(),
            prev_right: String::new(),
            sync_scroll_offset: egui::Vec2::ZERO,
            left_max_offset: egui::Vec2::ZERO,
            right_max_offset: egui::Vec2::ZERO,
            last_painted_styled_revision: 0,
            pending_right_apply: None,
            bench: bench_ctrl,
        }
    }

    #[cfg(not(feature = "bench"))]
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let theme = AppTheme::graphite();
        apply_theme(&cc.egui_ctx, &theme);
        Self {
            theme,
            left: Pane::default(),
            right: Pane::default(),
            pipeline: DiffPipeline::spawn(cc.egui_ctx.clone()),
            prev_left: String::new(),
            prev_right: String::new(),
            sync_scroll_offset: egui::Vec2::ZERO,
            left_max_offset: egui::Vec2::ZERO,
            right_max_offset: egui::Vec2::ZERO,
            last_painted_styled_revision: 0,
            pending_right_apply: None,
        }
    }

    fn detect_and_dispatch(&mut self) {
        if self.left.buffer != self.prev_left || self.right.buffer != self.prev_right {
            self.prev_left = self.left.buffer.clone();
            self.prev_right = self.right.buffer.clone();
            let revision = self
                .pipeline
                .dispatch(&self.left.buffer, &self.right.buffer);
            self.left.current_stamp = BufferStamp::from_text(revision, &self.left.buffer);
            self.right.current_stamp = BufferStamp::from_text(revision, &self.right.buffer);
        }
    }

    fn apply_diff_results(&mut self) {
        // Frames with a buffer at or above this size in bytes exceed the
        // 100 ms per-frame budget when both panes' galleys rebuild in the
        // same frame (each rebuild is ~85 ms for 310 KB on the reference
        // workstation). Below this threshold, a single back-to-back pair
        // of rebuilds comfortably fits in one frame, so staggering only
        // costs a frame of latency without any per-frame payoff. The cut
        // is empirical: 200 KB keeps edit_latency's 62 KB fixtures on
        // the fast path while sending large_paste's 310 KB case through
        // the staggered path.
        const STAGGER_THRESHOLD_BYTES: usize = 200_000;

        // Drain any right-pane apply that was deferred last frame before
        // looking for a new result. Consuming it first preserves
        // latest-wins semantics when a newer result also lands this
        // frame: the newer result's right portion replaces whatever
        // prior pending payload we just drained into the pane (the
        // prior payload is observably a subset of what the new one
        // will install, by monotonic revision).
        if let Some(pending) = self.pending_right_apply.take() {
            self.right.line_roles = pending.roles;
            self.right.inline_emphasis = pending.inline_emphasis;
            self.right.applied_revision = pending.revision;
            self.right.applied_stamp = pending.stamp;
        }

        if let Some(applied) = self.pipeline.drain_results() {
            // Latest-only apply for the left pane. A mismatched
            // roles/emphasis length or source stamp against the current
            // buffer identity is handled at paint time by
            // `build_layout_job`'s whole-pane neutral fallback.
            self.left.line_roles = applied.left_roles;
            self.left.inline_emphasis = applied.left_inline_emphasis;
            self.left.applied_revision = applied.revision;
            self.left.applied_stamp = applied.left_stamp;

            let large = self.left.buffer.len() >= STAGGER_THRESHOLD_BYTES
                || self.right.buffer.len() >= STAGGER_THRESHOLD_BYTES;
            if large {
                // Defer the right pane's apply by one frame so only one
                // pane's layouter rebuilds per frame on large buffers.
                self.pending_right_apply = Some(PendingRightApply {
                    roles: applied.right_roles,
                    inline_emphasis: applied.right_inline_emphasis,
                    revision: applied.revision,
                    stamp: applied.right_stamp,
                });
            } else {
                self.right.line_roles = applied.right_roles;
                self.right.inline_emphasis = applied.right_inline_emphasis;
                self.right.applied_revision = applied.revision;
                self.right.applied_stamp = applied.right_stamp;
            }
        }
    }

    /// Compute the new value of `last_painted_styled_revision` after a
    /// frame. Both panes must have taken the styled path this frame;
    /// the advanced value is the `min` of their applied revisions.
    /// Monotonic: never decreases.
    fn update_last_painted_styled_revision(&mut self) {
        if self.left.painted_styled_this_frame && self.right.painted_styled_this_frame {
            let candidate = self.left.applied_revision.min(self.right.applied_revision);
            if candidate > self.last_painted_styled_revision {
                self.last_painted_styled_revision = candidate;
            }
        }
    }
}

impl eframe::App for App {
    /// Outer clear color — the `gl.clearColor` value painted once per
    /// frame before any egui shape is submitted. With
    /// `with_decorations(false)` and a full-bleed title + central panel
    /// the clear color is only visible during sub-frame layout transients
    /// (resize, minimize/restore), but wiring `theme.bg_frame` here gives
    /// the matting token a real runtime read.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.bg_frame.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(feature = "bench")]
        let frame_entry = std::time::Instant::now();
        let ctx = ui.ctx().clone();

        // Reset per-frame flags before anything draws.
        self.left.painted_styled_this_frame = false;
        self.right.painted_styled_this_frame = false;

        self.apply_diff_results();

        // A deferred right-pane apply must be drained on the next frame
        // or it would sit indefinitely in non-bench builds (which only
        // repaint on demand). The bench harness already requests a
        // continuous repaint, so this is a no-op there.
        if self.pending_right_apply.is_some() {
            ctx.request_repaint();
        }

        #[cfg(feature = "bench")]
        if self.bench.is_some() {
            bench_before_draw(self, &ctx);
        }

        let theme = self.theme;
        draw_title_bar(ui, &theme, self.pipeline.is_worker_alive());

        // Hairline-divider variant of the Phase-1 panel layout. The
        // 50/50 ratio is enforced by sizing the left panel to half the
        // available central area width on each frame; `resizable(false)`
        // prevents the user from dragging the split away from 50/50
        // (the plan locks the ratio).
        let left_width = {
            let available = ui.available_rect_before_wrap().width();
            (available * 0.5).max(80.0)
        };
        // Panels use `bg_pane` fill with NO inner_margin so the
        // crosshatch texture (painted inside `render_pane`) covers the
        // full pane including the interior padding area. The 24/28 px
        // inset is applied inside render_pane via an inner Frame.
        let pane_frame = egui::Frame::NONE.fill(theme.bg_pane);
        // Clamp against the prior frame's known per-pane max before
        // egui's `ScrollArea::begin()` consumes the offset. Allowing a
        // shorter pane to see an out-of-range value breaks its
        // content-space hit-testing until `end()` clamps it back.
        let left_offset_in = clamp_sync_for_pane(self.sync_scroll_offset, self.left_max_offset);
        let right_offset_in = clamp_sync_for_pane(self.sync_scroll_offset, self.right_max_offset);
        let (left_offset_out, left_max_offset) = egui::Panel::left("pane_left_panel")
            .resizable(false)
            .exact_size(left_width)
            .frame(pane_frame)
            .show(ui, |ui| {
                render_pane(
                    ui,
                    "pane_left",
                    PaneSide::Left,
                    &mut self.left,
                    &theme,
                    left_offset_in,
                )
            })
            .inner;
        let (right_offset_out, right_max_offset) = egui::CentralPanel::default()
            .frame(pane_frame)
            .show(ui, |ui| {
                render_pane(
                    ui,
                    "pane_right",
                    PaneSide::Right,
                    &mut self.right,
                    &theme,
                    right_offset_in,
                )
            })
            .inner;
        let max_offset_changed =
            left_max_offset != self.left_max_offset || right_max_offset != self.right_max_offset;
        self.left_max_offset = left_max_offset;
        self.right_max_offset = right_max_offset;

        // Scroll-sync update: write `sync_scroll_offset` only when a pane's captured
        // offset *differs* from the pane-local value actually fed into
        // that pane this frame, and only from one pane per frame.
        // Left is still checked first so the resize-edge-case
        // arbitration from the amendment remains deterministic.
        if let Some(next_sync_offset) = decide_sync_write(
            left_offset_in,
            left_offset_out,
            right_offset_in,
            right_offset_out,
        ) {
            self.sync_scroll_offset = next_sync_offset;
            ctx.request_repaint();
        } else if max_offset_changed {
            ctx.request_repaint();
        }

        self.update_last_painted_styled_revision();
        self.detect_and_dispatch();

        #[cfg(feature = "bench")]
        if self.bench.is_some() {
            bench_after_draw(self, &ctx, frame_entry);
        }
    }
}

fn clamp_sync_for_pane(sync: egui::Vec2, pane_max: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        sync.x.clamp(0.0, pane_max.x.max(0.0)),
        sync.y.clamp(0.0, pane_max.y.max(0.0)),
    )
}

fn decide_sync_write(
    left_in: egui::Vec2,
    left_out: egui::Vec2,
    right_in: egui::Vec2,
    right_out: egui::Vec2,
) -> Option<egui::Vec2> {
    if left_out != left_in {
        Some(left_out)
    } else if right_out != right_in {
        Some(right_out)
    } else {
        None
    }
}

fn pane_max_offset(content_size: egui::Vec2, inner_size: egui::Vec2) -> egui::Vec2 {
    egui::vec2(
        (content_size.x - inner_size.x).max(0.0),
        (content_size.y - inner_size.y).max(0.0),
    )
}

fn render_pane(
    ui: &mut egui::Ui,
    id_prefix: &str,
    side: PaneSide,
    pane: &mut Pane,
    theme: &AppTheme,
    sync_scroll_offset: egui::Vec2,
) -> (egui::Vec2, egui::Vec2) {
    let font_id: FontId = TextStyle::Monospace.resolve(ui.style());
    let text_color: Color32 = ui.visuals().text_color();
    let pane_is_empty = pane.buffer.is_empty();
    let hint = match side {
        PaneSide::Left => "Paste or type original text…",
        PaneSide::Right => "Paste or type revised text…",
    };
    let Pane {
        buffer,
        line_roles,
        inline_emphasis,
        layout_cache,
        current_stamp,
        applied_stamp,
        painted_styled_this_frame,
        ..
    } = pane;
    let current_revision = current_stamp.revision;
    let source_stamp = *applied_stamp;
    let painted_styled_for_gutter = std::cell::Cell::new(false);

    let mut layouter =
        |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| -> Arc<Galley> {
            let text = buf.as_str();
            let text_hash = hash_text(text);
            let current_buffer_stamp =
                BufferStamp::from_text_hash(current_revision, text.len(), text_hash);
            let identity = PaintIdentity::new(current_buffer_stamp, source_stamp);
            let key = layout_cache_key(
                identity,
                hash_roles(line_roles),
                hash_inline_emphasis(inline_emphasis),
                hash_paint_inputs(theme, side, text_color),
                wrap_width.to_bits(),
            );
            if let Some(cache) = layout_cache.as_ref()
                && cache.key == key
            {
                painted_styled_for_gutter.set(cache.is_styled);
                // egui evicts galleys not used in the preceding pass, even if
                // we retain an Arc. Re-submit the job to keep its paragraph
                // cache alive, so the next edit only reshapes changed lines.
                return ui.fonts_mut(|fonts| fonts.layout_job((*cache.galley.job).clone()));
            }
            let is_styled = is_styled_paint_valid(text, line_roles, inline_emphasis, identity);
            let mut job: LayoutJob = build_layout_job(
                text,
                line_roles,
                inline_emphasis,
                LayoutPaint {
                    identity,
                    pane: side,
                    font_id: font_id.clone(),
                    text_color,
                    theme,
                },
            );
            job.wrap.max_width = wrap_width;
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            *layout_cache = Some(LayoutCache {
                key,
                galley: galley.clone(),
                is_styled,
            });
            painted_styled_for_gutter.set(is_styled);
            galley
        };

    #[cfg(feature = "bench")]
    let edit_id = match side {
        PaneSide::Left => bench::left_edit_id(),
        PaneSide::Right => bench::right_edit_id(),
    };

    // Pane interior padding: 24 px vertical. Horizontal is asymmetric:
    // 4 px left + 13 px gutter + 11 px gap + 28 px right = 56 px total
    // horizontal inset, with the TextEdit column starting 28 px in from
    // the pane edge. The gutter is placed *outside*
    // the `ScrollArea::both()` so it stays stationary under horizontal
    // text scroll; vertical sync is achieved by reading
    // `TextEditOutput::galley_pos` + each row's `pos.y`.
    let frame_response = egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 4,
            right: 28,
            top: 24,
            bottom: 24,
        })
        .show(ui, |ui| {
            let content_rect = ui.available_rect_before_wrap();
            let gutter_rect = egui::Rect::from_min_size(
                content_rect.min,
                egui::vec2(13.0, content_rect.height()),
            );
            let scroll_rect = egui::Rect::from_min_max(
                egui::pos2(gutter_rect.right() + 11.0, content_rect.top()),
                content_rect.max,
            );

            // Reserve the full content_rect in the parent ui so the
            // enclosing Frame sizes itself correctly — the actual
            // child ui below is placed absolutely at `scroll_rect`.
            ui.allocate_rect(content_rect, egui::Sense::hover());

            // Paint the crosshatch on the `scroll_rect` (the
            // `TextEdit` surface) before the `ScrollArea` renders so
            // the hatch sits under the text. `TextEdit`'s frame fill
            // was switched to `Color32::TRANSPARENT` in `apply_theme`
            // (via `visuals.text_edit_bg_color`), so the hatch shows
            // through the text field. The gutter and padding strip
            // outside `scroll_rect` read as a flat `theme.bg_pane`
            // fill inherited from the panel frame.
            let scroll_painter = ui.painter_at(scroll_rect);
            paint_crosshatch(&scroll_painter, scroll_rect, theme);

            // `scope_builder` returns an `InnerResponse<ScrollAreaOutput<TextEditOutput>>`.
            // Pull out the full `ScrollAreaOutput` so we can read both
            // `state.clamped_offset()` (for the sync-scroll sibling update) and
            // `inner` (the `TextEditOutput` the gutter needs).
            let scroll_output = ui
                .scope_builder(egui::UiBuilder::new().max_rect(scroll_rect), |ui| {
                    egui::ScrollArea::both()
                        .id_salt(format!("{id_prefix}_scroll"))
                        .scroll_offset(sync_scroll_offset)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut te = TextEdit::multiline(buffer)
                                .id_source(format!("{id_prefix}_edit"))
                                .font(TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(24)
                                .layouter(&mut layouter);
                            if pane_is_empty {
                                te = te.hint_text(
                                    egui::RichText::new(hint).color(theme.ink_dim).monospace(),
                                );
                            }
                            #[cfg(feature = "bench")]
                            let te = te.id(edit_id);
                            // Force the TextEdit to fill the full scroll
                            // viewport (both axes) so the clickable area
                            // matches the visible pane. TextEdit's
                            // intrinsic size is only `desired_rows *
                            // row_height` tall; without `vertical_justify`
                            // it leaves empty crosshatch below the text.
                            // Mirrors
                            // `Ui::add_sized` internals but calls
                            // `te.show(ui)` so we can reach into the
                            // `TextEditOutput` the gutter needs.
                            let avail = ui.available_size();
                            let layout =
                                egui::Layout::centered_and_justified(ui.layout().main_dir());
                            ui.allocate_ui_with_layout(avail, layout, |ui| te.show(ui))
                                .inner
                        })
                })
                .inner;
            let text_output = &scroll_output.inner;
            let captured_scroll_offset = scroll_output.state.clamped_offset();
            let max_scroll_offset =
                pane_max_offset(scroll_output.content_size, scroll_output.inner_rect.size());
            let styled_paint_valid = painted_styled_for_gutter.get()
                || (buffer.is_empty()
                    && is_styled_paint_valid(
                        buffer,
                        line_roles,
                        inline_emphasis,
                        PaintIdentity::new(
                            BufferStamp::from_text_hash(
                                current_revision,
                                buffer.len(),
                                hash_text(buffer),
                            ),
                            source_stamp,
                        ),
                    ));
            *painted_styled_this_frame = styled_paint_valid;

            paint_gutter(
                ui,
                gutter_rect,
                text_output,
                line_roles,
                GutterPaint {
                    side,
                    styled_paint_valid,
                    font_id: font_id.clone(),
                    theme,
                },
            );
            (captured_scroll_offset, max_scroll_offset)
        });

    // Bubble the pane's post-render scroll offset back to `App::ui`
    // plus the current pane max so `App::ui` can clamp the next
    // frame's shared input before egui consumes it.
    frame_response.inner
}

/// Paint a very faint crosshatch texture across `rect`, using
/// `theme.hatch_ink` (ink color at ~4% alpha) per the design spec's
/// `--hatch-ink` token. Two sets of diagonal lines (±45°) at 7 px
/// spacing match the spec's `repeating-linear-gradient(…, 1px 7px)`
/// pattern. Cost is ~`2 * (rect.width() + rect.height()) / 7` line
/// shapes per pane per frame.
fn paint_crosshatch(painter: &egui::Painter, rect: egui::Rect, theme: &AppTheme) {
    painter.rect_filled(rect, 0.0, theme.bg_pane);
    let stroke = egui::Stroke::new(1.0, theme.hatch_ink);
    let spacing = 7.0;
    let diag = rect.width() + rect.height();
    let mut offset = -rect.height();
    while offset <= diag {
        // Descending diagonal ("\"): line from (left, top + offset)
        // toward (left + diag, top + offset - diag)
        let x0 = rect.left();
        let y0 = rect.top() + offset;
        let x1 = rect.left() + diag;
        let y1 = rect.top() + offset - diag;
        // Clip to rect by intersecting manually-sliced endpoints
        let p0 = clip_to_rect(egui::pos2(x0, y0), egui::pos2(x1, y1), rect);
        if let Some((a, b)) = p0 {
            painter.line_segment([a, b], stroke);
        }
        // Ascending diagonal ("/"): line from (left, top - offset)
        // toward (left + diag, top - offset + diag)
        let x0 = rect.left();
        let y0 = rect.top() - offset + rect.height();
        let x1 = rect.left() + diag;
        let y1 = rect.top() - offset + rect.height() + diag;
        let p1 = clip_to_rect(egui::pos2(x0, y0), egui::pos2(x1, y1), rect);
        if let Some((a, b)) = p1 {
            painter.line_segment([a, b], stroke);
        }
        offset += spacing;
    }
}

/// Liang-Barsky style segment-to-rect clip. Returns the clipped
/// endpoints if any portion of the segment lies inside `rect`.
fn clip_to_rect(
    a: egui::Pos2,
    b: egui::Pos2,
    rect: egui::Rect,
) -> Option<(egui::Pos2, egui::Pos2)> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let mut t_min = 0.0f32;
    let mut t_max = 1.0f32;
    let edges = [
        (-dx, a.x - rect.left()),
        (dx, rect.right() - a.x),
        (-dy, a.y - rect.top()),
        (dy, rect.bottom() - a.y),
    ];
    for (p, q) in edges {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let t = q / p;
        if p < 0.0 {
            if t > t_max {
                return None;
            }
            if t > t_min {
                t_min = t;
            }
        } else {
            if t < t_min {
                return None;
            }
            if t < t_max {
                t_max = t;
            }
        }
    }
    Some((
        egui::pos2(a.x + t_min * dx, a.y + t_min * dy),
        egui::pos2(a.x + t_max * dx, a.y + t_max * dy),
    ))
}

/// Paint the per-pane gutter strip.
///
/// Layout: 13 px marker column positioned immediately left of the
/// TextEdit column, with an 11 px gap between them. Contents:
/// - A single marker glyph per non-equal row: `−` on the left pane,
///   `+` on the right pane, painted in the pane's `*_inline` hue
///   (`removed_inline` / `added_inline`) for full visual parity with
///   the diff's word-level emphasis.
///
/// Vertical alignment comes from `TextEditOutput::galley_pos` plus each
/// `PlacedRow::pos.y`, which together resolve to the screen-space top
/// of every row *after* the ScrollArea's scroll offset is applied.
/// Because the gutter is painted outside the ScrollArea, horizontal
/// text scroll leaves it stationary.
///
/// Stale-render fallback: the gutter mirrors `build_layout_job`'s
/// neutral-fallback invariant. When styled text paint was invalid or
/// `line_roles.len()` does not agree with `galley.rows.len()` (modulo a
/// trailing empty newline row), the column paints blank. This is
/// intentional: paired with the neutral TextEdit fallback, a stale frame
/// shows no tint *and* no gutter marker rather than risking a stale or
/// misaligned marker.
fn paint_gutter(
    ui: &egui::Ui,
    gutter_rect: egui::Rect,
    text_output: &egui::widgets::text_edit::TextEditOutput,
    line_roles: &[Role],
    paint: GutterPaint<'_>,
) {
    let painter = ui.painter_at(gutter_rect);
    let galley = &text_output.galley;
    let n = line_roles.len();
    let rows = galley.rows.len();
    // Accepted alignments: rows == n (normal) or rows == n + 1 (the
    // trailing empty row egui appends when text ends with `\n`, which
    // `segment()` — and thus `line_roles` — deliberately does not
    // include). Anything else is a stale render; fall back to blank.
    if !should_paint_gutter_markers(paint.styled_paint_valid, rows, n) {
        return;
    }

    let marker_char = match paint.side {
        PaneSide::Left => '−',
        PaneSide::Right => '+',
    };
    let marker_color = gutter_marker_color(paint.theme, paint.side);
    // `Fonts::layout_no_wrap` is memoized, so repeated calls with the
    // same `(text, font, color)` tuple return the cached Arc. Layout
    // cost is a single hash lookup per pane per frame.
    let marker_galley =
        ui.fonts_mut(|f| f.layout_no_wrap(marker_char.to_string(), paint.font_id, marker_color));
    let marker_size = marker_galley.size();
    let column_width = gutter_rect.width();
    let marker_x = gutter_rect.left() + (column_width - marker_size.x) * 0.5;

    for (row, role) in galley.rows.iter().zip(line_roles.iter()) {
        if gutter_marker_for_role(paint.side, *role).is_none() {
            continue;
        }
        let row_top = text_output.galley_pos.y + row.pos.y;
        let row_height = row.size.y;
        // Cull rows that fall entirely outside the gutter's clip band
        // (scrolled above or below the visible viewport).
        if row_top + row_height < gutter_rect.top() || row_top > gutter_rect.bottom() {
            continue;
        }
        let marker_y = row_top + (row_height - marker_size.y) * 0.5;
        painter.galley(
            egui::pos2(marker_x, marker_y),
            marker_galley.clone(),
            marker_color,
        );
    }
}

struct GutterPaint<'a> {
    side: PaneSide,
    styled_paint_valid: bool,
    font_id: FontId,
    theme: &'a AppTheme,
}

fn should_paint_gutter_markers(styled_paint_valid: bool, rows: usize, role_count: usize) -> bool {
    styled_paint_valid && (rows == role_count || rows == role_count + 1)
}

/// Draw the 34 px custom title bar. Native OS decorations are disabled
/// (`with_decorations(false)`), so this bar IS the chrome — it is not
/// a placeholder. Contents: brand mark (16×16 diagonal-split rounded
/// square painted with the active theme's `removed_inline` /
/// `added_inline` pair so the identity mark mirrors the diff palette
/// users see on highlighted lines), the app name "Fast Diff Viewer",
/// and minimize/maximize/close controls wired to
/// `ViewportCommand::Minimized` / `Maximized` / `Close`. A full-width
/// invisible drag surface behind the row forwards pointer-drags to
/// `ViewportCommand::StartDrag` for OS window drag. No version badge
/// (the design spec annotation is explicit: "No version number").
///
/// Not yet wired: resize handles - `with_decorations(false)` removes
/// the native edge affordances. Adding resize would be a product-design
/// change because it introduces new pointer handling around pane surfaces.
fn draw_title_bar(ui: &mut egui::Ui, theme: &AppTheme, worker_alive: bool) {
    let ctx = ui.ctx().clone();
    egui::Panel::top("title_bar")
        .exact_size(34.0)
        .frame(
            egui::Frame::NONE
                .fill(theme.bg_app)
                .inner_margin(egui::Margin::symmetric(10, 6)),
        )
        .show(ui, |ui| {
            // Full-width invisible drag surface beneath the row
            // content. Windows decorations are off, so clicking any
            // non-button area of this strip starts an OS window drag.
            let title_rect = ui.max_rect();
            let drag_response = ui.interact(
                title_rect,
                egui::Id::unique("title_bar_drag"),
                egui::Sense::drag(),
            );
            if drag_response.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            ui.horizontal_centered(|ui| {
                // Brand mark.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                draw_brand_mark(ui.painter(), rect, theme, worker_alive);

                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("Fast Diff Viewer")
                        .color(theme.ink)
                        .size(12.5),
                );
                // When the diff worker has died (panic or
                // channel break), surface the failure inline with the
                // product name so the user can tell that diffing has
                // stopped without reading a log or restarting. Uses the
                // active theme's `removed_inline` token — no new
                // operation-scoped color is introduced.
                if !worker_alive {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("• diff offline")
                            .color(theme.removed_inline)
                            .size(12.5),
                    );
                }
                // No version badge — the spec is explicit: "No version
                // number".

                // Min/max/close pinned to the right edge. Buttons sit
                // on top of the drag surface so their clicks take
                // precedence over the background drag.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // The Unicode fallback `×` (U+00D7) renders
                    // with a thin stroke and compact bearing, so at the
                    // shared 12 pt it reads smaller than its `—` / `□`
                    // siblings. Upsize the close glyph only (16 pt)
                    // while keeping the 28 × 22 hit target constant so
                    // the three controls read as a coherent Windows-11
                    // triplet without shifting chrome layout.
                    if title_bar_button(ui, "×", 16.0, theme).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    let is_maxed = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                    if title_bar_button(ui, if is_maxed { "❐" } else { "□" }, 12.0, theme).clicked()
                    {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_maxed));
                    }
                    if title_bar_button(ui, "—", 12.0, theme).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                });
            });

            // Bottom hairline separator.
            let y = ui.max_rect().bottom();
            ui.painter().hline(
                ui.max_rect().x_range(),
                y - 0.5,
                egui::Stroke::new(1.0, theme.rule),
            );
        });
}

fn title_bar_button(ui: &mut egui::Ui, glyph: &str, size: f32, theme: &AppTheme) -> egui::Response {
    let btn = egui::Button::new(egui::RichText::new(glyph).color(theme.ink).size(size))
        .frame(false)
        .min_size(egui::vec2(28.0, 22.0));
    ui.add(btn)
}

fn draw_brand_mark(
    painter: &egui::Painter,
    rect: egui::Rect,
    theme: &AppTheme,
    worker_alive: bool,
) {
    // The brand mark tracks the active theme's diff inline-emphasis
    // palette so the identity mark always carries the same colors the
    // user sees on highlighted diff lines: `removed_inline` on the
    // top-left half, `added_inline` on the bottom-right half. On
    // Paper the pair is pink/green (`#E9C2CB` / `#BFE2C8`); on
    // Graphite it is dark red-brown / dark forest. The design choice
    // is "brand mirrors the diff" — not "brand is a fixed Paper
    // accent overlaid on every theme".
    //
    // When the diff worker is dead, drop both halves to the
    // theme's `ink_dim` (disabled-text) token so the mark visibly
    // desaturates — the brand stops "mirroring the diff" because there
    // is no diff being computed. No new color token is introduced.
    let corner = rect.width() * (48.0 / 256.0);
    let (left_fill, right_fill) = if worker_alive {
        (theme.removed_inline, theme.added_inline)
    } else {
        (theme.ink_dim, theme.ink_dim)
    };
    painter.rect_filled(rect, corner, left_fill);
    // Bottom-right triangle filled with `added_inline`. Split runs
    // along the anti-diagonal (top-right → bottom-left, 135° in CSS).
    // Triangle vertices are the top-right, bottom-right, and
    // bottom-left corners — the half below/right of the split line.
    let tri = vec![rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    painter.add(egui::Shape::convex_polygon(
        tri,
        right_fill,
        egui::Stroke::NONE,
    ));
    // 1px hairline ring in the active theme's ink at ~15% opacity.
    let ring_color =
        Color32::from_rgba_unmultiplied(theme.ink.r(), theme.ink.g(), theme.ink.b(), 38);
    painter.rect_stroke(
        rect,
        corner,
        egui::Stroke::new(1.0, ring_color),
        egui::StrokeKind::Inside,
    );
}

#[cfg(feature = "bench")]
fn bench_before_draw(app: &mut App, ctx: &egui::Context) {
    use crate::bench::{BenchState, EditPhase, PastePhase};
    use std::time::Instant;

    // Keep repaints flowing so the state machine is driven by the
    // compositor regardless of input activity.
    ctx.request_repaint();

    let bench = app.bench.as_mut().expect("guarded by is_some()");
    match &mut bench.state {
        BenchState::ColdStart { .. } => {
            // Nothing to do pre-draw: cold start only observes T1 after
            // the first paint completes.
        }
        BenchState::EditLatency(state) => match state.phase {
            EditPhase::LoadFixtures => {
                app.left.buffer = bench.left_fixture.clone();
                app.right.buffer = bench.right_fixture.clone();
                state.phase = EditPhase::WaitSettle;
                state.settle_frames = 0;
            }
            EditPhase::WaitSettle => {
                state.settle_frames += 1;
            }
            EditPhase::RequestFocus => {
                ctx.memory_mut(|m| m.request_focus(bench::left_edit_id()));
            }
            EditPhase::Injecting => {
                // Verify focus is where we think it is; otherwise abort.
                let focused = ctx.memory(|m| m.focused());
                if focused != Some(bench::left_edit_id()) {
                    eprintln!(
                        "bench: focus lost before injection (expected left edit id, got {focused:?})"
                    );
                    state.phase = EditPhase::Finalize;
                    return;
                }
                if state.inject_this_frame
                    && (state.next_probe_id as u32) < state.target_probe_count
                {
                    let rev_before = app.pipeline.doc_revision;
                    let t0 = Instant::now();
                    ctx.input_mut(|inp| inp.events.push(egui::Event::Text("x".into())));
                    state.probes.push(bench::Probe {
                        id: state.next_probe_id,
                        // revision after the event will be rev_before + 1
                        // once detect_and_dispatch observes the buffer change
                        // at end-of-update.
                        revision: rev_before + 1,
                        t0,
                        t1: None,
                    });
                    state.next_probe_id += 1;
                }
                state.inject_this_frame = !state.inject_this_frame;
            }
            EditPhase::Finalize => {}
        },
        BenchState::LargePaste(state) => match state.phase {
            PastePhase::LoadFixtures => {
                app.left.buffer = bench.left_fixture.clone();
                app.right.buffer = bench.right_fixture.clone();
                state.phase = PastePhase::WaitSettle;
                state.settle_frames = 0;
            }
            PastePhase::WaitSettle => {
                state.settle_frames += 1;
            }
            PastePhase::RequestFocus => {
                ctx.memory_mut(|m| m.request_focus(bench::right_edit_id()));
            }
            PastePhase::Injecting => {
                let focused = ctx.memory(|m| m.focused());
                if focused != Some(bench::right_edit_id()) {
                    eprintln!(
                        "bench: focus lost before paste (expected right edit id, got {focused:?})"
                    );
                    state.phase = PastePhase::Finalize;
                    return;
                }
                let rev_before = app.pipeline.doc_revision;
                let t_paste = Instant::now();
                let payload = bench.paste_payload.clone();
                ctx.input_mut(|inp| inp.events.push(egui::Event::Paste(payload)));
                state.paste_revision = Some(rev_before + 1);
                state.t_paste = Some(t_paste);
                state.phase = PastePhase::Recording;
            }
            PastePhase::Recording | PastePhase::Finalize => {}
        },
    }
}

#[cfg(feature = "bench")]
fn bench_after_draw(app: &mut App, ctx: &egui::Context, frame_entry: std::time::Instant) {
    use crate::bench::{BenchState, EditPhase, PastePhase};
    use std::time::Instant;

    let frame_us = frame_entry.elapsed().as_micros() as u64;
    let bench = app.bench.as_mut().expect("guarded by is_some()");
    let out_dir = bench.config.out_dir.clone();
    let trial = bench.config.trial;

    match &mut bench.state {
        BenchState::ColdStart {
            frames_painted,
            t1,
            finished,
        } => {
            *frames_painted += 1;
            // Both panes were drawn this frame; treat this as the first
            // frame that painted both `TextEdit` panes.
            if t1.is_none() && *frames_painted >= 1 {
                *t1 = Some(Instant::now());
            }
            if !*finished && let Some(t1_instant) = *t1 {
                let t0 = bench::BENCH_T0.get().copied().unwrap_or(frame_entry);
                // Use absolute ns since a process epoch (approximate).
                let t0_ns = 0u128;
                let t1_ns = t1_instant.saturating_duration_since(t0).as_nanos();
                if let Err(e) = bench::write_cold_start_csv(&out_dir, trial, t0_ns, t1_ns) {
                    eprintln!("bench: cold_start csv write failed: {e}");
                }
                *finished = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        BenchState::EditLatency(state) => {
            // Resolve probes whose revision has now been painted.
            for p in state.probes.iter_mut() {
                if p.t1.is_none() && p.revision <= app.last_painted_styled_revision {
                    p.t1 = Some(Instant::now());
                }
            }
            // Phase advance for settle → focus → inject.
            match state.phase {
                EditPhase::WaitSettle => {
                    // Wait until both panes are caught up to the
                    // dispatched revision AND the buffer is non-empty
                    // (fixtures loaded).
                    let settled = app.pipeline.last_applied_revision >= app.pipeline.doc_revision
                        && app.pipeline.doc_revision > 0
                        && app.last_painted_styled_revision >= app.pipeline.doc_revision;
                    if settled && state.settle_frames >= 2 {
                        state.phase = EditPhase::RequestFocus;
                    }
                }
                EditPhase::RequestFocus => {
                    // Focus was requested pre-draw this frame; one
                    // no-op frame elapses, then we move to Injecting
                    // next frame.
                    state.phase = EditPhase::Injecting;
                }
                EditPhase::Injecting => {
                    let resolved = state.probes.iter().filter(|p| p.t1.is_some()).count() as u32;
                    if resolved >= state.target_probe_count {
                        state.phase = EditPhase::Finalize;
                    }
                }
                EditPhase::Finalize | EditPhase::LoadFixtures => {}
            }
            if state.phase == EditPhase::Finalize && !state.written {
                if let Err(e) = bench::write_edit_latency_csv(&out_dir, &state.probes) {
                    eprintln!("bench: edit_latency csv write failed: {e}");
                }
                state.written = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        BenchState::LargePaste(state) => {
            // Capture frame timing for the 120-frame window after paste.
            if matches!(state.phase, PastePhase::Recording) {
                let frame_id = state.frame_times.len() as u32;
                state.frame_times.push((frame_id, frame_us));
                // Capture settle when the paste revision has been painted.
                if let (Some(paste_rev), None) = (state.paste_revision, state.t_applied)
                    && app.last_painted_styled_revision >= paste_rev
                {
                    state.t_applied = Some(Instant::now());
                }
                if state.frame_times.len() as u32 >= state.target_frames {
                    state.phase = PastePhase::Finalize;
                }
            }
            // Phase transitions for load → settle → focus → inject.
            match state.phase {
                PastePhase::WaitSettle => {
                    let settled = app.pipeline.last_applied_revision >= app.pipeline.doc_revision
                        && app.pipeline.doc_revision > 0
                        && app.last_painted_styled_revision >= app.pipeline.doc_revision;
                    if settled && state.settle_frames >= 2 {
                        state.phase = PastePhase::RequestFocus;
                    }
                }
                PastePhase::RequestFocus => {
                    state.phase = PastePhase::Injecting;
                }
                _ => {}
            }
            if state.phase == PastePhase::Finalize && !state.written {
                let t_paste = state.t_paste.unwrap_or(frame_entry);
                let t_applied = state.t_applied.unwrap_or_else(Instant::now);
                if let Err(e) =
                    bench::write_large_paste_csvs(&out_dir, &state.frame_times, t_paste, t_applied)
                {
                    eprintln!("bench: large_paste csv write failed: {e}");
                }
                state.written = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_sync_for_pane, decide_sync_write, layout_cache_key, pane_max_offset,
        should_paint_gutter_markers,
    };
    use crate::buffer_stamp::BufferStamp;
    use crate::render::PaintIdentity;
    use eframe::egui;

    #[test]
    fn no_change_returns_none() {
        let left_in = egui::vec2(12.0, 34.0);
        let right_in = egui::vec2(56.0, 78.0);

        assert_eq!(
            decide_sync_write(left_in, left_in, right_in, right_in),
            None
        );
    }

    #[test]
    fn left_user_scroll_propagates() {
        let left_in = egui::vec2(12.0, 34.0);
        let left_out = egui::vec2(12.0, 40.0);
        let right_in = egui::vec2(56.0, 34.0);

        assert_eq!(
            decide_sync_write(left_in, left_out, right_in, right_in),
            Some(left_out)
        );
    }

    #[test]
    fn right_user_scroll_propagates() {
        let left_in = egui::vec2(12.0, 34.0);
        let right_in = egui::vec2(56.0, 78.0);
        let right_out = egui::vec2(60.0, 78.0);

        assert_eq!(
            decide_sync_write(left_in, left_in, right_in, right_out),
            Some(right_out)
        );
    }

    #[test]
    fn left_priority_on_simultaneous_change() {
        let left_in = egui::vec2(12.0, 34.0);
        let left_out = egui::vec2(18.0, 34.0);
        let right_in = egui::vec2(56.0, 78.0);
        let right_out = egui::vec2(56.0, 82.0);

        assert_eq!(
            decide_sync_write(left_in, left_out, right_in, right_out),
            Some(left_out)
        );
    }

    #[test]
    fn clamps_y_when_sync_exceeds_pane_max_y() {
        assert_eq!(
            clamp_sync_for_pane(egui::vec2(10.0, 50.0), egui::vec2(20.0, 40.0)),
            egui::vec2(10.0, 40.0)
        );
    }

    #[test]
    fn clamps_x_when_sync_exceeds_pane_max_x() {
        assert_eq!(
            clamp_sync_for_pane(egui::vec2(50.0, 10.0), egui::vec2(40.0, 20.0)),
            egui::vec2(40.0, 10.0)
        );
    }

    #[test]
    fn zero_pane_max_clamps_to_zero() {
        assert_eq!(
            clamp_sync_for_pane(egui::vec2(50.0, 10.0), egui::Vec2::ZERO),
            egui::Vec2::ZERO
        );
    }

    #[test]
    fn negative_max_is_treated_as_zero() {
        assert_eq!(
            clamp_sync_for_pane(egui::vec2(50.0, 10.0), egui::vec2(-5.0, -1.0)),
            egui::Vec2::ZERO
        );
    }

    #[test]
    fn pane_max_offset_floors_negative_components() {
        assert_eq!(
            pane_max_offset(egui::vec2(50.0, 10.0), egui::vec2(60.0, 5.0)),
            egui::vec2(0.0, 5.0)
        );
    }

    #[test]
    fn gutter_markers_require_identity_valid_styled_paint() {
        assert!(should_paint_gutter_markers(true, 3, 3));
        assert!(should_paint_gutter_markers(true, 4, 3));
        assert!(
            !should_paint_gutter_markers(false, 3, 3),
            "same-row-count stale text must leave the gutter blank"
        );
        assert!(!should_paint_gutter_markers(true, 5, 3));
    }

    #[test]
    fn layout_cache_key_changes_when_source_stamp_changes() {
        let current = BufferStamp::from_text(2, "alpha BETA\nsame tail");
        let stale_source = BufferStamp::from_text(1, "alpha beta\nsame tail");
        let fresh_source = current;

        assert_ne!(
            layout_cache_key(PaintIdentity::new(current, stale_source), 11, 22, 33, 44),
            layout_cache_key(PaintIdentity::new(current, fresh_source), 11, 22, 33, 44),
            "neutral stale-paint cache entries must not survive once the matching source stamp lands"
        );
    }
}
