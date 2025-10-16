use chrono::{Local, TimeZone};
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use journal::file::JournalFileMap;
use journal::index::{Bitmap, FileIndex};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    symbols,
    text::Span,
    widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, ListState},
};
use std::collections::{HashMap, HashSet};
use std::io;

/// Application state for interactive visualization
struct AppState {
    /// List of field names extracted from the journal file
    fields: Vec<String>,
    /// Currently selected field index
    selected_field: usize,
    /// Values for the currently selected field (lazily loaded)
    current_field_values: Vec<String>,
    /// Currently selected value index for the current field
    selected_value: usize,
    /// Which panel is currently focused (0 = fields, 1 = values)
    focused_panel: usize,
    /// List state for fields list (tracks scroll position)
    fields_list_state: ListState,
    /// List state for values list (tracks scroll position)
    values_list_state: ListState,
}

impl AppState {
    fn new(journal_file: &JournalFileMap) -> io::Result<Self> {
        // Scan journal file to get all unique field names
        let fields = Self::scan_field_names(journal_file)?;

        let mut fields_list_state = ListState::default();
        if !fields.is_empty() {
            fields_list_state.select(Some(0));
        }

        Ok(Self {
            fields,
            selected_field: 0,
            current_field_values: Vec::new(),
            selected_value: 0,
            focused_panel: 0,
            fields_list_state,
            values_list_state: ListState::default(),
        })
    }

    fn scan_field_names(journal_file: &JournalFileMap) -> io::Result<Vec<String>> {
        use journal::file::JournalReader;

        let mut reader = JournalReader::default();
        let mut field_names = HashSet::new();

        // Iterate through all field objects in the journal
        while let Some(field_guard) = reader.fields_enumerate(journal_file)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        {
            let field_name = String::from_utf8_lossy(&field_guard.payload).to_string();
            field_names.insert(field_name);
        }

        let mut fields: Vec<String> = field_names.into_iter().collect();
        fields.sort();
        Ok(fields)
    }

    fn load_field_values(
        &mut self,
        journal_file: &JournalFileMap,
    ) -> io::Result<()> {
        use journal::file::JournalReader;

        // Clone the field name to avoid borrow issues
        let field = match self.current_field() {
            Some(f) => f.clone(),
            None => return Ok(()),
        };

        let mut reader = JournalReader::default();
        let mut values = HashSet::new();

        // Query all unique values for this field
        reader.field_data_query_unique(journal_file, field.as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Enumerate all data objects for this field
        while let Some(data_guard) = reader.field_data_enumerate(journal_file)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
        {
            // Use the payload_bytes() method to extract bytes
            let payload_bytes = data_guard.payload_bytes();
            let value = String::from_utf8_lossy(payload_bytes).to_string();
            values.insert(value);
        }

        self.current_field_values = values.into_iter().collect();
        self.current_field_values.sort();
        self.selected_value = 0;

        // Update values list state
        self.values_list_state = ListState::default();
        if !self.current_field_values.is_empty() {
            self.values_list_state.select(Some(0));
        }

        Ok(())
    }

    fn current_field(&self) -> Option<&String> {
        self.fields.get(self.selected_field)
    }

    fn current_values(&self) -> &[String] {
        &self.current_field_values
    }

    fn current_field_value_pair(&self) -> Option<(String, String)> {
        if let Some(field) = self.current_field() {
            if let Some(value) = self.current_field_values.get(self.selected_value) {
                return Some((field.clone(), value.clone()));
            }
        }
        None
    }

    fn next_field(&mut self, journal_file: &JournalFileMap) -> io::Result<()> {
        if !self.fields.is_empty() {
            self.selected_field = (self.selected_field + 1) % self.fields.len();
            self.fields_list_state.select(Some(self.selected_field));
            self.load_field_values(journal_file)?;
        }
        Ok(())
    }

    fn previous_field(&mut self, journal_file: &JournalFileMap) -> io::Result<()> {
        if !self.fields.is_empty() {
            self.selected_field = if self.selected_field == 0 {
                self.fields.len() - 1
            } else {
                self.selected_field - 1
            };
            self.fields_list_state.select(Some(self.selected_field));
            self.load_field_values(journal_file)?;
        }
        Ok(())
    }

    fn next_value(&mut self) {
        if !self.current_field_values.is_empty() {
            self.selected_value = (self.selected_value + 1) % self.current_field_values.len();
            self.values_list_state.select(Some(self.selected_value));
        }
    }

    fn previous_value(&mut self) {
        if !self.current_field_values.is_empty() {
            self.selected_value = if self.selected_value == 0 {
                self.current_field_values.len() - 1
            } else {
                self.selected_value - 1
            };
            self.values_list_state.select(Some(self.selected_value));
        }
    }

    fn toggle_focus(&mut self) {
        self.focused_panel = (self.focused_panel + 1) % 2;
    }
}

/// Build a bitmap of entry indices that contain a specific field-value pair
/// This scans through all entries in the journal file
fn build_bitmap_for_field_value(
    journal_file: &JournalFileMap,
    _file_index: &FileIndex,
    field_name: &str,
    value: &str,
) -> io::Result<Bitmap> {
    use journal::file::{JournalReader, offset_array::Direction};
    use roaring::RoaringBitmap;

    // Build a map from entry offset to entry index
    let mut entry_offset_to_index: HashMap<std::num::NonZeroU64, u32> = HashMap::new();

    // We need to iterate through all entries to build the mapping
    let mut reader = JournalReader::default();
    reader.set_location(journal::file::cursor::Location::Head);

    let mut entry_index = 0u32;
    while reader.step(journal_file, Direction::Forward)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?
    {
        if let Ok(offset) = reader.get_entry_offset() {
            entry_offset_to_index.insert(offset, entry_index);
            entry_index += 1;
        }
    }

    // Now scan through the data objects for this field
    let mut matching_indices = Vec::new();

    let field_data_iterator = journal_file.field_data_objects(field_name.as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    for data_object_result in field_data_iterator {
        let data_object = data_object_result
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        // Check if this data object has the matching value
        let payload_bytes = data_object.payload_bytes();
        let data_value = String::from_utf8_lossy(payload_bytes);

        if data_value == value {
            // Get the inlined cursor to find which entries contain this data object
            if let Some(inlined_cursor) = data_object.inlined_cursor() {
                let mut entry_offsets = Vec::new();
                if inlined_cursor.collect_offsets(journal_file, &mut entry_offsets).is_ok() {
                    // Map offsets to indices
                    for entry_offset in entry_offsets {
                        if let Some(&index) = entry_offset_to_index.get(&entry_offset) {
                            matching_indices.push(index);
                        }
                    }
                }
            }
        }
    }

    // Sort and deduplicate
    matching_indices.sort_unstable();
    matching_indices.dedup();

    // Create bitmap
    let rb = RoaringBitmap::from_sorted_iter(matching_indices.into_iter())
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok(Bitmap(rb))
}

fn format_timestamp_medium(micros: u64) -> String {
    let secs = (micros / 1_000_000) as i64;
    let nanos = ((micros % 1_000_000) * 1000) as u32;

    if let Some(dt) = Local.timestamp_opt(secs, nanos).single() {
        dt.format("%H:%M %d/%m").to_string()
    } else {
        // Fallback if timestamp is invalid
        "??:?? ??/??".to_string()
    }
}

pub fn visualize_histogram_interactive(
    journal_file: &JournalFileMap,
    file_index: &FileIndex,
    _histogram_data: &[(u64, u32)],
    _title: String,
) -> io::Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app_state = AppState::new(journal_file)?;

    // Load values for the initial field
    if !app_state.fields.is_empty() {
        app_state.load_field_values(journal_file)?;
    }

    // Calculate initial histogram data
    let (mut histogram_data, mut chart_title) = if let Some((field, value)) = app_state.current_field_value_pair() {
        let key = format!("{}={}", field, value);
        if let Some(bitmap) = file_index.entries_index.get(&key) {
            let data = file_index.file_histogram.from_bitmap(bitmap);
            (data, format!("Histogram: {}", key))
        } else {
            match build_bitmap_for_field_value(journal_file, file_index, &field, &value) {
                Ok(bitmap) => {
                    let data = file_index.file_histogram.from_bitmap(&bitmap);
                    (data, format!("Histogram: {}={}", field, value))
                }
                Err(e) => {
                    (vec![], format!("Error building histogram: {}", e))
                }
            }
        }
    } else {
        (vec![], "Select a field and value".to_string())
    };

    let mut needs_redraw = true;

    // Main event loop
    loop {
        // Only recalculate histogram if state changed
        if needs_redraw {
            (histogram_data, chart_title) = if let Some((field, value)) = app_state.current_field_value_pair() {
                // Look up the field=value combination in the index
                let key = format!("{}={}", field, value);
                if let Some(bitmap) = file_index.entries_index.get(&key) {
                    // Use pre-indexed bitmap
                    let data = file_index.file_histogram.from_bitmap(bitmap);
                    (data, format!("Histogram: {}", key))
                } else {
                    // Build bitmap dynamically
                    match build_bitmap_for_field_value(journal_file, file_index, &field, &value) {
                        Ok(bitmap) => {
                            let data = file_index.file_histogram.from_bitmap(&bitmap);
                            (data, format!("Histogram: {}={}", field, value))
                        }
                        Err(e) => {
                            (vec![], format!("Error building histogram: {}", e))
                        }
                    }
                }
            } else {
                (vec![], "Select a field and value".to_string())
            };

            terminal.draw(|f| {
                // Create main layout: horizontal split
                let main_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
                    .split(f.area());

                // Left side: Histogram
                render_histogram(f, main_chunks[0], &histogram_data, &chart_title);

                // Right side: Split into two vertical panels
                let menu_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(main_chunks[1]);

                // Top right: Fields list
                render_fields_list(f, menu_chunks[0], &mut app_state);

                // Bottom right: Values list
                render_values_list(f, menu_chunks[1], &mut app_state);
            })?;

            needs_redraw = false;
        }

        // Wait for an event (blocks until event arrives)
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Tab => {
                    app_state.toggle_focus();
                    needs_redraw = true;
                }
                KeyCode::Up => {
                    if app_state.focused_panel == 0 {
                        app_state.previous_field(journal_file)?;
                    } else {
                        app_state.previous_value();
                    }
                    needs_redraw = true;
                }
                KeyCode::Down => {
                    if app_state.focused_panel == 0 {
                        app_state.next_field(journal_file)?;
                    } else {
                        app_state.next_value();
                    }
                    needs_redraw = true;
                }
                _ => {}
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn render_histogram(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    histogram_data: &[(u64, u32)],
    title: &str,
) {
    if histogram_data.is_empty() {
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::White));
        f.render_widget(block, area);
        return;
    }

    // Convert histogram data to chart points
    let data: Vec<(f64, f64)> = histogram_data
        .iter()
        .map(|(bucket_seconds, count)| (*bucket_seconds as f64, *count as f64))
        .collect();

    let min_x = data.first().map(|(x, _)| *x).unwrap_or(0.0);
    let max_x = data.last().map(|(x, _)| *x).unwrap_or(1.0);
    let max_y = data
        .iter()
        .map(|(_, y)| *y)
        .max_by(|a, b| a.partial_cmp(b).unwrap())
        .unwrap_or(1.0);

    // Create X-axis labels with timestamps
    let num_labels = 7.min(histogram_data.len());
    let step = if histogram_data.len() > 1 {
        (histogram_data.len() - 1) / (num_labels - 1).max(1)
    } else {
        1
    };

    let x_labels: Vec<Span> = (0..num_labels)
        .map(|i| {
            let idx = (i * step).min(histogram_data.len() - 1);
            let (bucket_seconds, _) = histogram_data[idx];
            let timestamp_micros = bucket_seconds * 1_000_000;
            Span::styled(
                format_timestamp_medium(timestamp_micros),
                Style::default().fg(Color::Yellow),
            )
        })
        .collect();

    // Create Y-axis labels
    let y_step = (max_y / 5.0).max(1.0);
    let y_labels: Vec<Span> = (0..=5)
        .map(|i| {
            Span::styled(
                format!("{:.0}", i as f64 * y_step),
                Style::default().fg(Color::Yellow),
            )
        })
        .collect();

    // Create dataset
    let dataset = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&data);

    // Create chart
    let chart = Chart::new(vec![dataset])
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::White)),
        )
        .x_axis(
            Axis::default()
                .title("Time")
                .style(Style::default().fg(Color::Gray))
                .labels(x_labels)
                .bounds([min_x, max_x]),
        )
        .y_axis(
            Axis::default()
                .title("Count")
                .style(Style::default().fg(Color::Gray))
                .labels(y_labels)
                .bounds([0.0, max_y * 1.1]),
        );

    f.render_widget(chart, area);
}

fn render_fields_list(f: &mut ratatui::Frame, area: ratatui::layout::Rect, app_state: &mut AppState) {
    let items: Vec<ListItem> = app_state
        .fields
        .iter()
        .map(|field| ListItem::new(field.as_str()))
        .collect();

    let border_style = if app_state.focused_panel == 0 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };

    let highlight_style = if app_state.focused_panel == 0 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title("Fields [Tab to switch, ↑↓ to navigate, q to quit]")
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_style(highlight_style)
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, area, &mut app_state.fields_list_state);
}

fn render_values_list(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    app_state: &mut AppState,
) {
    // Clone values to avoid borrowing issues
    let values: Vec<String> = app_state.current_values().to_vec();
    let is_empty = values.is_empty();
    let focused_panel = app_state.focused_panel;
    let title = app_state.current_field().map(|f| format!("Values for: {}", f)).unwrap_or_else(|| "Values".to_string());

    let items: Vec<ListItem> = if is_empty {
        vec![ListItem::new("(loading...)").style(Style::default().fg(Color::Gray))]
    } else {
        values
            .iter()
            .map(|value| ListItem::new(value.as_str()))
            .collect()
    };

    let border_style = if focused_panel == 1 {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::White)
    };

    let highlight_style = if focused_panel == 1 {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_style(highlight_style)
        .highlight_symbol(">> ");

    f.render_stateful_widget(list, area, &mut app_state.values_list_state);
}
