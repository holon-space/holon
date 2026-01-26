/// Configurable layout and typography settings for the GPUI renderer.
/// All builders read from this struct instead of using hardcoded constants.
#[derive(Clone, Debug)]
pub struct LayoutStyle {
    // Tree items
    pub tree_indent_px: f32,
    pub tree_chevron_size: f32,
    pub tree_bullet_size: f32,
    pub tree_item_min_height: f32,
    pub tree_chevron_font_size: f32,

    // Text
    pub text_default_size: f32,
    pub text_line_height: f32,

    // Section headings
    pub section_title_size: f32,
    pub section_title_weight: gpui::FontWeight,
    pub section_gap: f32,
    pub section_padding_bottom: f32,

    // Badge/tags
    pub badge_font_size: f32,
    pub badge_padding_x: f32,
    pub badge_padding_y: f32,

    // Layout
    pub sidebar_width: f32,
    pub content_padding_x: f32,
    pub content_padding_y: f32,
    pub sidebar_padding_x: f32,
    pub sidebar_padding_y: f32,

    // Icons (used by state_toggle, icon, checkbox)
    pub icon_size: f32,
    pub icon_box_padding: f32,

    // Card
    pub card_border_radius: f32,
    pub card_padding_x: f32,
    pub card_padding_y: f32,
    pub card_gap: f32,
    pub card_border_width: f32,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        Self {
            tree_indent_px: 28.0,
            tree_chevron_size: 20.0,
            tree_bullet_size: 7.0,
            tree_item_min_height: 32.0,
            tree_chevron_font_size: 10.0,

            text_default_size: 15.0,
            text_line_height: 26.0,

            section_title_size: 28.0,
            section_title_weight: gpui::FontWeight::BOLD,
            section_gap: 8.0,
            section_padding_bottom: 8.0,

            badge_font_size: 11.0,
            badge_padding_x: 8.0,
            badge_padding_y: 2.0,

            sidebar_width: 260.0,
            content_padding_x: 40.0,
            content_padding_y: 12.0,
            sidebar_padding_x: 12.0,
            sidebar_padding_y: 8.0,

            icon_size: 16.0,
            icon_box_padding: 4.0,

            card_border_radius: 10.0,
            card_padding_x: 14.0,
            card_padding_y: 10.0,
            card_gap: 4.0,
            card_border_width: 4.0,
        }
    }
}
