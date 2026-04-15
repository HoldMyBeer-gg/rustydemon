use crate::app::CascExplorerApp;

/// Draw the right-panel details/preview area.
pub fn draw_preview(ui: &mut egui::Ui, app: &mut CascExplorerApp) {
    ui.heading("Details / Preview");
    ui.separator();

    if app.selected.is_none() {
        ui.label("Select a file to preview it.");
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("preview_pane_outer")
        .auto_shrink([false, false])
        .show(ui, |ui| draw_preview_body(ui, app));
}

fn draw_preview_body(ui: &mut egui::Ui, app: &mut CascExplorerApp) {
    let Some(sel) = &app.selected else {
        return;
    };

    // ── File metadata ──────────────────────────────────────────────────────────
    egui::Grid::new("preview_meta")
        .num_columns(2)
        .spacing([4.0, 2.0])
        .show(ui, |ui| {
            let name = sel
                .result
                .filename
                .as_deref()
                .and_then(|p| p.rsplit(['/', '\\']).next())
                .unwrap_or("(unknown)");

            meta_row(ui, "Name:", name);

            let ext = name.rsplit('.').next().unwrap_or("").to_uppercase();
            meta_row(ui, "Type:", &ext);

            if let Some(id) = sel.result.file_data_id {
                meta_row(ui, "FDID:", &id.to_string());
            }

            meta_row(ui, "Hash:", &format!("{:016X}", sel.result.hash));
            meta_row(ui, "CKey:", &sel.result.ckey.to_hex()[..16]);
            meta_row(ui, "Locale:", &format!("{:?}", sel.result.locale));

            if let Some(data) = &sel.data {
                meta_row(ui, "Size:", &format_size(data.len()));
            }
        });

    ui.separator();

    // ── Error ──────────────────────────────────────────────────────────────────
    if let Some(err) = &sel.load_error {
        ui.colored_label(egui::Color32::from_rgb(220, 80, 80), format!("⚠ {err}"));
        return;
    }

    // ── Loading spinner while the background task is running ──────────────────
    if sel.data.is_none() && sel.load_error.is_none() && app.loading {
        ui.spinner();
        ui.label("Loading file data…");
        return;
    }

    // ── Viewer override dropdown ──────────────────────────────────────────────
    // Lets the user force a specific preview plugin on files whose
    // format isn't auto-detected.  Great for reverse-engineering new
    // formats: try the BC-texture decoder on a mystery blob, see if
    // anything recognisable comes out.  Deferred because we can't
    // mutate `app` while `sel` is borrowed.
    let mut override_change: Option<Option<usize>> = None;
    if sel.data.is_some() {
        let names = crate::preview::plugin_names();
        let current_label = match sel.preview_override {
            None => "Auto".to_string(),
            Some(i) => names.get(i).cloned().unwrap_or_else(|| format!("#{i}")),
        };
        ui.horizontal(|ui| {
            ui.label("Viewer:");
            egui::ComboBox::from_id_salt("preview_override")
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(sel.preview_override.is_none(), "Auto")
                        .clicked()
                    {
                        override_change = Some(None);
                    }
                    for (i, name) in names.iter().enumerate() {
                        if ui
                            .selectable_label(sel.preview_override == Some(i), name.as_str())
                            .clicked()
                        {
                            override_change = Some(Some(i));
                        }
                    }
                });
        });
        ui.separator();
    }

    // ── Plugin-provided preview ───────────────────────────────────────────────
    if let Some(preview) = &sel.preview {
        // Texture (inline image).
        if let Some(tex) = &preview.texture {
            let tex_size = tex.size_vec2();
            let max_w = ui.available_width();
            let scale = (max_w / tex_size.x).min(1.0);
            let display_size = tex_size * scale;
            ui.image((tex.id(), display_size));
            ui.separator();
        }

        // 3D mesh viewport (currently only WMO group geometry).
        if let Some(mesh) = preview.mesh3d.clone() {
            crate::viewport3d::paint_mesh(ui, mesh);
            ui.separator();
        }

        // Text block (formatted summary or full text file). The outer
        // ScrollArea handles overflow; this widget just lays out as tall
        // as its contents.
        if let Some(text) = &preview.text {
            ui.add(
                egui::TextEdit::multiline(&mut text.as_str())
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
            ui.separator();
        }
    } else if let Some(data) = &sel.data {
        // ── Hex-dump fallback (no plugin claimed this file) ───────────────────
        let preview_len = data.len().min(256);
        let hex: String = data[..preview_len]
            .chunks(16)
            .enumerate()
            .map(|(i, row)| {
                let hex_part: String = row.iter().map(|b| format!("{b:02X} ")).collect();
                let ascii_part: String = row
                    .iter()
                    .map(|&b| {
                        if b.is_ascii_graphic() || b == b' ' {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                format!("{:04X}  {hex_part:<48}  {ascii_part}", i * 16)
            })
            .collect::<Vec<_>>()
            .join("\n");

        ui.add(
            egui::TextEdit::multiline(&mut hex.as_str())
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
    }

    // ── Deep-search content matches ────────────────────────────────────────────
    if !sel.content_matches.is_empty() {
        ui.separator();
        ui.label(format!(
            "Deep search: {} entries",
            sel.content_matches.len()
        ));
        for m in &sel.content_matches {
            ui.label(
                egui::RichText::new(format!("[{}] {}", m.kind, m.inner_path))
                    .small()
                    .monospace(),
            );
        }
    }

    ui.separator();

    // ── PCX palette picker (for SC1 assets with external palettes) ───────────
    let is_pcx = sel
        .result
        .filename
        .as_deref()
        .map(|n| n.to_ascii_lowercase().ends_with(".pcx"))
        .unwrap_or(false);

    // Deferred actions: we can't mutate `app` while `sel` (a shared borrow
    // of `app.selected`) is live, so collect intents here and run them after.
    let mut pcx_load_palette = false;
    let mut pcx_clear_palette = false;
    let mut export_action_clicked: Option<usize> = None;
    let mut export_raw_clicked = false;

    if is_pcx {
        ui.horizontal(|ui| {
            if ui
                .button("Load Palette…")
                .on_hover_text("Apply an external .pal/.wpe palette to this PCX")
                .clicked()
            {
                pcx_load_palette = true;
            }
            if app.pcx_palette.is_some() && ui.button("Clear").clicked() {
                pcx_clear_palette = true;
            }
            if let Some(name) = &app.pcx_palette_name {
                ui.label(
                    egui::RichText::new(format!("Palette: {name}"))
                        .small()
                        .color(egui::Color32::from_gray(180)),
                );
            }
        });
        ui.separator();
    }

    // ── Export buttons ─────────────────────────────────────────────────────────
    if sel.data.is_some() {
        ui.horizontal(|ui| {
            // Plugin-provided exports (e.g. "Export As PNG", "Export As BK2").
            if let Some(preview) = &sel.preview {
                for (i, action) in preview.extra_exports.iter().enumerate() {
                    if ui.button(action.label).clicked() {
                        export_action_clicked = Some(i);
                    }
                }
            }
            if ui.button("Export Raw").clicked() {
                export_raw_clicked = true;
            }
        });
    }

    // Drop the `sel` immutable borrow before touching `app` mutably.
    let _ = sel;

    if let Some(new_override) = override_change {
        apply_preview_override(app, new_override, ui.ctx());
    }
    if pcx_load_palette {
        load_pcx_palette(app, ui.ctx());
    }
    if pcx_clear_palette {
        app.pcx_palette = None;
        app.pcx_palette_name = None;
        reload_current_pcx(app, ui.ctx());
    }
    if let Some(idx) = export_action_clicked {
        // Re-borrow the action: it lives on `app.selected.preview.extra_exports`.
        let action = app
            .selected
            .as_ref()
            .and_then(|s| s.preview.as_ref())
            .and_then(|p| p.extra_exports.get(idx))
            .cloned();
        if let Some(a) = action {
            run_plugin_export(app, &a);
        }
    }
    if export_raw_clicked {
        export_raw(app);
    }
}

/// Apply a new viewer override to the current selection and re-run
/// the preview pipeline.  `new_override` is `None` for auto-dispatch
/// or `Some(index)` to force a specific plugin from the registry.
fn apply_preview_override(
    app: &mut CascExplorerApp,
    new_override: Option<usize>,
    ctx: &egui::Context,
) {
    // Snapshot everything we need before we take any borrows that
    // would conflict with `app.handler`.
    let (data, filename) = {
        let Some(sel) = app.selected.as_ref() else {
            return;
        };
        let Some(data) = sel.data.clone() else {
            return;
        };
        (data, sel.result.filename.clone())
    };

    // Multi-file plugins (WMO → groups, M2 → textures) need a sibling
    // fetcher; the override path gives them the same one the initial
    // dispatch uses.
    let handler_ref = app.handler.as_ref();
    let by_name = |path: &str| -> Option<Vec<u8>> { handler_ref?.open_file_by_name(path).ok() };
    let by_fdid = |id: u32| -> Option<Vec<u8>> { handler_ref?.open_file_by_fdid(id).ok() };
    let siblings = crate::preview::SiblingFetcher {
        by_name: &by_name,
        by_fdid: &by_fdid,
    };

    let new_preview = match new_override {
        Some(idx) => {
            crate::preview::run_with_override(idx, filename.as_deref(), &data, ctx, &siblings)
        }
        None => crate::preview::run(filename.as_deref(), &data, ctx, &siblings),
    };

    if let Some(sel) = app.selected.as_mut() {
        sel.preview_override = new_override;
        sel.preview = new_preview;
    }
}

fn load_pcx_palette(app: &mut CascExplorerApp, ctx: &egui::Context) {
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Palette", &["pal", "wpe", "act"])
        .add_filter("Any", &["*"])
        .pick_file()
    else {
        return;
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            app.status = format!("Palette read failed: {e}");
            return;
        }
    };
    let Some(palette) = crate::preview::pcx::parse_palette_file(&bytes) else {
        app.status = "Palette format not recognised".into();
        return;
    };
    app.pcx_palette_name = path.file_name().and_then(|n| n.to_str()).map(String::from);
    app.pcx_palette = Some(palette);
    reload_current_pcx(app, ctx);
}

fn reload_current_pcx(app: &mut CascExplorerApp, ctx: &egui::Context) {
    let Some(sel) = app.selected.as_mut() else {
        return;
    };
    let Some(data) = sel.data.clone() else {
        return;
    };
    let is_pcx = sel
        .result
        .filename
        .as_deref()
        .map(|n| n.to_ascii_lowercase().ends_with(".pcx"))
        .unwrap_or(false);
    if !is_pcx {
        return;
    }
    if let Some(pal) = app.pcx_palette.as_deref() {
        crate::app::apply_pcx_palette_override(sel, &data, pal, ctx);
    } else {
        // Rebuild with default decoder
        // No sibling fetcher on PCX reload — palette swaps don't trigger
        // multi-file plugins.
        let no_name = |_: &str| -> Option<Vec<u8>> { None };
        let no_fdid = |_: u32| -> Option<Vec<u8>> { None };
        let siblings = crate::preview::SiblingFetcher {
            by_name: &no_name,
            by_fdid: &no_fdid,
        };
        sel.preview = crate::preview::run(sel.result.filename.as_deref(), &data, ctx, &siblings);
    }
}

fn run_plugin_export(app: &CascExplorerApp, action: &crate::preview::ExportAction) {
    let Some(sel) = &app.selected else { return };
    let Some(data) = &sel.data else { return };

    let stem = sel
        .result
        .filename
        .as_deref()
        .and_then(|n| std::path::Path::new(n).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("export");

    let Some(path) = rfd::FileDialog::new()
        .set_file_name(format!("{stem}.{}", action.default_extension))
        .add_filter(action.filter_name, &[action.default_extension])
        .save_file()
    else {
        return;
    };

    match (action.build)(data) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, &bytes) {
                rfd::MessageDialog::new()
                    .set_title("Export failed")
                    .set_description(format!("Could not write {}: {e}", path.display()))
                    .set_level(rfd::MessageLevel::Warning)
                    .show();
            }
        }
        Err(e) => {
            rfd::MessageDialog::new()
                .set_title("Export failed")
                .set_description(format!("{}: {e}", action.label))
                .set_level(rfd::MessageLevel::Warning)
                .show();
        }
    }
}

fn export_raw(app: &CascExplorerApp) {
    let Some(sel) = &app.selected else { return };
    let Some(data) = &sel.data else { return };

    let default_name = sel
        .result
        .filename
        .as_deref()
        .and_then(|n| n.rsplit(['/', '\\']).next())
        .unwrap_or("export.bin");

    if let Some(path) = rfd::FileDialog::new()
        .set_file_name(default_name)
        .save_file()
    {
        let _ = std::fs::write(&path, data);
    }
}

fn meta_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).strong());
    // Truncate long values to prevent the panel from expanding.
    let max_chars = 24;
    if value.len() > max_chars {
        let truncated = format!("{}…", &value[..max_chars]);
        let resp = ui.label(&truncated);
        resp.on_hover_text(value);
    } else {
        ui.label(value);
    }
    ui.end_row();
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}
