use crate::app::CascExplorerApp;

/// Draw the right-panel details/preview area.
pub fn draw_preview(ui: &mut egui::Ui, app: &mut CascExplorerApp) {
    ui.heading("Details / Preview");
    ui.separator();

    let Some(sel) = &app.selected else {
        ui.label("Select a file to preview it.");
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

        // Text block (formatted summary or full text file).
        if let Some(text) = &preview.text {
            egui::ScrollArea::vertical()
                .id_salt("plugin_text_preview")
                .max_height(300.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut text.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                });
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

        egui::ScrollArea::vertical()
            .max_height(160.0)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut hex.as_str())
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                );
            });
    }

    // ── Deep-search content matches ────────────────────────────────────────────
    if !sel.content_matches.is_empty() {
        ui.separator();
        ui.label(format!(
            "Deep search: {} entries",
            sel.content_matches.len()
        ));
        egui::ScrollArea::vertical()
            .id_salt("content_matches")
            .max_height(120.0)
            .show(ui, |ui| {
                for m in &sel.content_matches {
                    ui.label(
                        egui::RichText::new(format!("[{}] {}", m.kind, m.inner_path))
                            .small()
                            .monospace(),
                    );
                }
            });
    }

    ui.separator();

    // ── Export buttons ─────────────────────────────────────────────────────────
    if sel.data.is_some() {
        ui.horizontal(|ui| {
            // Plugin-provided exports (e.g. "Export As PNG", "Export As BK2").
            if let Some(preview) = &sel.preview {
                for action in &preview.extra_exports {
                    if ui.button(action.label).clicked() {
                        run_plugin_export(app, action);
                    }
                }
            }
            if ui.button("Export Raw").clicked() {
                export_raw(app);
            }
        });
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
