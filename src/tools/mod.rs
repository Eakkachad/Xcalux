use crate::canvas::BlendMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolId {
    Brush,
    Eraser,
    Fill,
    Gradient,
    RectSelect,
    EllipseSelect,
    Lasso,
    PolygonLasso,
    MagicWand,
    Move,
    Transform,
    ColorPicker,
    Hand,
    Zoom,
    RotateView,
    Line,
    Shape,
    Reference,
}

impl ToolId {
    pub fn name(&self) -> &'static str {
        match self {
            ToolId::Brush => "Brush",
            ToolId::Eraser => "Eraser",
            ToolId::Fill => "Fill",
            ToolId::Gradient => "Gradient",
            ToolId::RectSelect => "Rect Select",
            ToolId::EllipseSelect => "Ellipse Select",
            ToolId::Lasso => "Lasso",
            ToolId::PolygonLasso => "Polygon Lasso",
            ToolId::MagicWand => "Magic Wand",
            ToolId::Move => "Move",
            ToolId::Transform => "Transform",
            ToolId::ColorPicker => "Color Picker",
            ToolId::Hand => "Hand",
            ToolId::Zoom => "Zoom",
            ToolId::RotateView => "Rotate View",
            ToolId::Line => "Line",
            ToolId::Shape => "Shape",
            ToolId::Reference => "Reference",
        }
    }

    pub fn shortcut(&self) -> &'static str {
        match self {
            ToolId::Brush => "B",
            ToolId::Eraser => "E",
            ToolId::Fill => "G",
            ToolId::Gradient => "Shift+G",
            ToolId::RectSelect => "M",
            ToolId::EllipseSelect => "Shift+M",
            ToolId::Lasso => "L",
            ToolId::PolygonLasso => "Shift+L",
            ToolId::MagicWand => "W",
            ToolId::Move => "V",
            ToolId::Transform => "Ctrl+T",
            ToolId::ColorPicker => "I",
            ToolId::Hand => "Space",
            ToolId::Zoom => "Z",
            ToolId::RotateView => "R",
            ToolId::Line => "U",
            ToolId::Shape => "U",
            ToolId::Reference => "",
        }
    }
}
