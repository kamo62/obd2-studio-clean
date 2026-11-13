use std::ops::Range;

use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, IntoElement, KeyBinding, LayoutId, PaintQuad, Pixels,
    Render, ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, actions, div, fill,
    hsla, relative, rgb,
};

actions!(
    text_field,
    [Backspace, Delete, Left, Right, Home, End, Paste]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, None),
        KeyBinding::new("delete", Delete, None),
        KeyBinding::new("left", Left, None),
        KeyBinding::new("right", Right, None),
        KeyBinding::new("home", Home, None),
        KeyBinding::new("end", End, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("ctrl-v", Paste, None),
    ]);
}

pub struct TextField {
    pub focus_handle: FocusHandle,
    pub content: SharedString,
    pub placeholder: SharedString,
    cursor: usize,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextField {
    pub fn new(cx: &mut Context<Self>, value: &str, placeholder: &str) -> Self {
        let content: SharedString = value.to_string().into();
        let cursor = content.len();
        Self {
            focus_handle: cx.focus_handle(),
            content,
            placeholder: placeholder.to_string().into(),
            cursor,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
        }
    }

    pub fn value(&self) -> String {
        self.content.to_string()
    }

    #[allow(dead_code)]
    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        let value = value.into().replace(['\n', '\r'], " ");
        self.content = value.into();
        self.cursor = self.content.len();
        self.marked_range = None;
        cx.notify();
    }

    fn safe_boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while offset > 0 && !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        let offset = self.safe_boundary(offset);
        self.content[..offset]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .map_or(0, |index| index)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        let offset = self.safe_boundary(offset);
        self.content[offset..]
            .chars()
            .next()
            .map(|character| offset + character.len_utf8())
            .map_or(self.content.len(), |index| index)
    }

    fn move_cursor(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.cursor = self.safe_boundary(offset);
        cx.notify();
    }

    fn replace_range(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let start = self.safe_boundary(range.start);
        let end = self.safe_boundary(range.end.max(start));
        let text = text.replace('\n', " ").replace('\r', " ");
        let mut value =
            String::with_capacity(start + text.len() + self.content.len().saturating_sub(end));
        value.push_str(&self.content[..start]);
        value.push_str(&text);
        value.push_str(&self.content[end..]);
        self.content = value.into();
        self.cursor = start + text.len();
        self.marked_range = None;
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let start = self.previous_boundary(self.cursor);
        self.replace_range(start..self.cursor, "", cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let end = self.next_boundary(self.cursor);
        self.replace_range(self.cursor..end, "", cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(self.previous_boundary(self.cursor), cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(self.next_boundary(self.cursor), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(self.content.len(), cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_range(self.cursor..self.cursor, &text, cx);
        }
    }

    fn utf8_offset_for_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_offset = 0;
        for character in self.content.chars() {
            if utf16_offset >= offset {
                break;
            }
            let next = utf16_offset + character.len_utf16();
            if next > offset {
                break;
            }
            utf16_offset = next;
            utf8_offset += character.len_utf8();
        }
        utf8_offset
    }

    fn utf16_offset_for_utf8(&self, offset: usize) -> usize {
        let offset = self.safe_boundary(offset);
        self.content[..offset].chars().map(char::len_utf16).sum()
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.utf8_offset_for_utf16(range_utf16.start)
            ..self.utf8_offset_for_utf16(range_utf16.end);
        actual_range.replace(
            self.utf16_offset_for_utf8(range.start)..self.utf16_offset_for_utf8(range.end),
        );
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let cursor = self.utf16_offset_for_utf8(self.cursor);
        Some(UTF16Selection {
            range: cursor..cursor,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| {
            self.utf16_offset_for_utf8(range.start)..self.utf16_offset_for_utf8(range.end)
        })
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|range| {
                self.utf8_offset_for_utf16(range.start)..self.utf8_offset_for_utf16(range.end)
            })
            .or_else(|| self.marked_range.clone())
            .unwrap_or(self.cursor..self.cursor);
        self.replace_range(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|range| {
                self.utf8_offset_for_utf16(range.start)..self.utf8_offset_for_utf16(range.end)
            })
            .or_else(|| self.marked_range.clone())
            .unwrap_or(self.cursor..self.cursor);
        let start = self.safe_boundary(range.start);
        self.replace_range(range, text, cx);
        if !text.is_empty() {
            let end = start + text.replace('\n', " ").replace('\r', " ").len();
            self.marked_range = Some(start..end.min(self.content.len()));
        }
        if let Some(selected_range) = selected_range_utf16 {
            let selected_start = start + self.utf8_offset_for_utf16(selected_range.start);
            let selected_end = start + self.utf8_offset_for_utf16(selected_range.end);
            self.cursor = self.safe_boundary(selected_end.max(selected_start));
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let start = self.utf8_offset_for_utf16(range_utf16.start);
        let end = self.utf8_offset_for_utf16(range_utf16.end);
        Some(Bounds::from_corners(
            gpui::point(bounds.left() + line.x_for_index(start), bounds.top()),
            gpui::point(bounds.left() + line.x_for_index(end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

struct TextElement {
    input: Entity<TextField>,
}

struct TextPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = TextPrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let display_text = if content.is_empty() {
            input.placeholder.clone()
        } else {
            content
        };
        let style = window.text_style();
        let text_color = if input.content.is_empty() {
            hsla(0.0, 0.0, 0.55, 1.0)
        } else {
            style.color
        };
        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &[run], None);
        let cursor_position = line.x_for_index(input.cursor.min(input.content.len()));
        let cursor = fill(
            Bounds::new(
                gpui::point(bounds.left() + cursor_position, bounds.top()),
                gpui::size(gpui::px(1.0), bounds.size.height),
            ),
            rgb(0x62d6ff),
        );
        TextPrepaintState {
            line: Some(line),
            cursor: Some(cursor),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(line) = prepaint.line.take() {
            let _ = line.paint(bounds.origin, window.line_height(), window, cx);
            if focus_handle.is_focused(window) {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
            let _ = self.input.update(cx, |input, _| {
                input.last_layout = Some(line);
                input.last_bounds = Some(bounds);
            });
        }
    }
}

impl Render for TextField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .w_full()
            .h(gpui::px(34.0))
            .items_center()
            .px_2()
            .bg(rgb(0x111923))
            .border_1()
            .border_color(rgb(0x344557))
            .rounded_sm()
            .key_context("TextField")
            .track_focus(&self.focus_handle(cx))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, _| this.focus_handle.focus(window)),
            )
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .child(
                div()
                    .w_full()
                    .text_size(gpui::px(14.0))
                    .child(TextElement { input: cx.entity() }),
            )
    }
}
