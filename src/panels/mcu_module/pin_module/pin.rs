use eframe::egui;

pub const PIN_FONT_SIZE: f32 = 10.0;
pub const PIN_ROUNDING: f32 = 0.0;

pub struct Pin {
    pub name: String,
    pub number: usize,
    pub reserved: bool,
    // rect: egui::Rect,
}

impl Pin {
    pub fn new(number: usize, name: &str) -> Self {
        Self {
            number,
            name: name.to_owned(),
            reserved: false,
        }
    }

    pub fn new_reserved(number: usize, name: &str) -> Self {
        Self {
            number,
            name: name.to_owned(),
            reserved: true,
        }
    }

    pub fn get_backgroung_collor(&self) -> egui::Color32 {
        match self.reserved {
            true => match self.name.as_str() {
                "VDD" | "VDDA" => egui::Color32::RED,
                "VBAT" => egui::Color32::LIGHT_RED,
                "VSS" | "VSSA" => egui::Color32::BLACK,
                _ => egui::Color32::LIGHT_GRAY,
            },
            false => egui::Color32::LIGHT_BLUE,
        }
    }
    pub fn get_text_collor(&self) -> egui::Color32 {
        match self.reserved {
            true => match self.name.as_str().trim() {
                "VSS" | "VSSA" => egui::Color32::WHITE,
                _ => egui::Color32::BLACK,
            },
            false => egui::Color32::BLACK,
        }
    }

    pub fn draw_vertical_text(&self, painter: &egui::Painter, pos: egui::Pos2) {
        let galley = painter.layout_no_wrap(
            self.name.to_owned(),
            egui::FontId::monospace(PIN_FONT_SIZE),
            egui::Color32::BLACK,
        );

        let text_shape = egui::epaint::TextShape {
            pos,
            galley,
            underline: egui::epaint::Stroke::NONE, //Default::default(),
            override_text_color: Some(self.get_text_collor()),
            angle: -std::f32::consts::FRAC_PI_2,
            fallback_color: egui::Color32::BLACK,
            opacity_factor: 1.0,
        };

        let shape = egui::Shape::Text(text_shape);
        painter.add(shape);
    }

    pub fn draw_horizontal_text(&self, painter: &egui::Painter, pos: egui::Pos2) {
        painter.text(
            pos,
            egui::Align2::LEFT_CENTER,
            self.name.as_str(),
            egui::FontId::monospace(PIN_FONT_SIZE),
            self.get_text_collor(),
        );
    }

    pub fn draw_right(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        heigt: f32,
        width: f32,
        ui: Option<&egui::Ui>,
    ) -> egui::Rect {
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(heigt, width));

        _ = painter.rect_filled(rect, PIN_ROUNDING, self.get_backgroung_collor());

        if let Some(ui) = ui {
            self.listen(&painter, ui, rect);
        }

        self.draw_horizontal_text(&painter, egui::pos2(rect.left() + 2.0, rect.center().y));

        rect
    }

    pub fn draw_left(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        heigt: f32,
        width: f32,
        ui: Option<&egui::Ui>,
    ) -> egui::Rect {
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(heigt, width));

        _ = painter.rect_filled(rect, PIN_ROUNDING, self.get_backgroung_collor());

        if let Some(ui) = ui {
            self.listen(&painter, ui, rect);
        }

        self.draw_horizontal_text(&painter, egui::pos2(rect.left() + 2.0, rect.center().y));

        rect
    }

    pub fn draw_top(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        heigt: f32,
        width: f32,
        ui: Option<&egui::Ui>,
    ) -> egui::Rect {
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, heigt));

        _ = painter.rect_filled(rect, PIN_ROUNDING, self.get_backgroung_collor());

        if let Some(ui) = ui {
            self.listen(&painter, ui, rect);
        }

        self.draw_vertical_text(&painter, egui::pos2(x + (width / 3.4), y + heigt - 4.0));

        rect
    }

    pub fn draw_bottom(
        &self,
        painter: &egui::Painter,
        x: f32,
        y: f32,
        heigt: f32,
        width: f32,
        ui: Option<&egui::Ui>,
    ) -> egui::Rect {
        self.draw_vertical_text(&painter, egui::pos2(x + (width / 3.4), y + heigt - 4.0));

        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, heigt));

        _ = painter.rect_filled(rect, PIN_ROUNDING, self.get_backgroung_collor());

        if let Some(ui) = ui {
            self.listen(&painter, ui, rect);
        }

        self.draw_vertical_text(&painter, egui::pos2(x + (width / 3.4), y + heigt - 4.0));

        rect
    }
}
