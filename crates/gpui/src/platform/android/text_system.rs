// Android text system using cosmic-text for shaping and swash for rasterization
// Based on the Linux CosmicTextSystem implementation

use crate::{
    Bounds, DevicePixels, Font, FontFeatures, FontId, FontMetrics, FontRun, FontStyle, FontWeight,
    GlyphId, LineLayout, Pixels, PlatformTextSystem, Point, RenderGlyphParams, SUBPIXEL_VARIANTS_X,
    SUBPIXEL_VARIANTS_Y, ShapedGlyph, ShapedRun, SharedString, Size, TextRenderingMode, point,
    size,
};
use anyhow::{Context as _, Ok, Result};
use collections::HashMap;
use cosmic_text::{
    Attrs, AttrsList, Family, Font as CosmicTextFont, FontFeatures as CosmicFontFeatures,
    FontSystem, ShapeBuffer, ShapeLine,
};

use itertools::Itertools;
use parking_lot::RwLock;
use smallvec::SmallVec;
use std::{borrow::Cow, sync::Arc};
use swash::{
    scale::{Render, ScaleContext, Source, StrikeWith},
    zeno::{Format, Transform, Vector},
};

pub(crate) struct CosmicTextSystem(RwLock<CosmicTextSystemState>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FontKey {
    family: SharedString,
    features: FontFeatures,
}

impl FontKey {
    fn new(family: SharedString, features: FontFeatures) -> Self {
        Self { family, features }
    }
}

struct CosmicTextSystemState {
    font_system: FontSystem,
    scratch: ShapeBuffer,
    swash_scale_context: ScaleContext,
    loaded_fonts: Vec<LoadedFont>,
    font_ids_by_family_cache: HashMap<FontKey, SmallVec<[FontId; 4]>>,
}

struct LoadedFont {
    font: Arc<CosmicTextFont>,
    features: CosmicFontFeatures,
    is_known_emoji_font: bool,
}

impl CosmicTextSystem {
    pub(crate) fn new() -> Self {
        let font_system = FontSystem::new();
        Self(RwLock::new(CosmicTextSystemState {
            font_system,
            scratch: ShapeBuffer::default(),
            swash_scale_context: ScaleContext::new(),
            loaded_fonts: Vec::new(),
            font_ids_by_family_cache: HashMap::default(),
        }))
    }
}

impl Default for CosmicTextSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformTextSystem for CosmicTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        self.0.write().add_fonts(fonts)
    }

    fn all_font_names(&self) -> Vec<String> {
        let mut result = self
            .0
            .read()
            .font_system
            .db()
            .faces()
            .filter_map(|face| face.families.first().map(|family| family.0.clone()))
            .collect_vec();
        result.sort();
        result.dedup();
        result
    }

    fn font_id(&self, font: &Font) -> Result<FontId> {
        let mut state = self.0.write();
        let key = FontKey::new(font.family.clone(), font.features.clone());

        // Try to get from cache or load the family
        let candidates = if let Some(font_ids) = state.font_ids_by_family_cache.get(&key) {
            font_ids.clone()
        } else {
            let font_ids = state.load_family(&font.family, &font.features)?;
            state.font_ids_by_family_cache.insert(key, font_ids.clone());
            font_ids
        };

        // Fallback chain: requested family -> Roboto -> DroidSans -> any loaded font
        let candidates = if !candidates.is_empty() {
            candidates
        } else {
            state.load_fallback_fonts(&font.features)?
        };

        if candidates.is_empty() {
            anyhow::bail!(
                "No fonts available for family '{}' or any fallbacks",
                font.family
            );
        }

        let best_ix = state.find_best_match(&candidates, font);
        Ok(candidates[best_ix])
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        let metrics = self
            .0
            .read()
            .loaded_font(font_id)
            .font
            .as_swash()
            .metrics(&[]);

        FontMetrics {
            units_per_em: metrics.units_per_em as u32,
            ascent: metrics.ascent,
            descent: -metrics.descent,
            line_gap: metrics.leading,
            underline_position: metrics.underline_offset,
            underline_thickness: metrics.stroke_size,
            cap_height: metrics.cap_height,
            x_height: metrics.x_height,
            bounding_box: Bounds {
                origin: point(0.0, 0.0),
                size: size(metrics.max_width, metrics.ascent + metrics.descent),
            },
        }
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        let lock = self.0.read();
        let glyph_metrics = lock.loaded_font(font_id).font.as_swash().glyph_metrics(&[]);
        let glyph_id = glyph_id.0 as u16;
        Ok(Bounds {
            origin: point(0.0, 0.0),
            size: size(
                glyph_metrics.advance_width(glyph_id),
                glyph_metrics.advance_height(glyph_id),
            ),
        })
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        self.0.read().advance(font_id, glyph_id)
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.0.read().glyph_for_char(font_id, ch)
    }

    fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        self.0.write().raster_bounds(params)
    }

    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        self.0.write().rasterize_glyph(params, raster_bounds)
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.0.write().layout_line(text, font_size, runs)
    }

    fn recommended_rendering_mode(
        &self,
        _font_id: FontId,
        _font_size: Pixels,
    ) -> TextRenderingMode {
        // On Android, grayscale rendering is typically preferred for mobile displays
        TextRenderingMode::Grayscale
    }
}

impl CosmicTextSystemState {
    fn loaded_font(&self, font_id: FontId) -> &LoadedFont {
        &self.loaded_fonts[font_id.0]
    }

    #[profiling::function]
    fn add_fonts(&mut self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        let db = self.font_system.db_mut();
        for bytes in fonts {
            match bytes {
                Cow::Borrowed(embedded_font) => db.load_font_data(embedded_font.to_vec()),
                Cow::Owned(bytes) => db.load_font_data(bytes),
            }
        }
        Ok(())
    }

    /// Load fallback fonts when the requested family is not found
    fn load_fallback_fonts(&mut self, features: &FontFeatures) -> Result<SmallVec<[FontId; 4]>> {
        // Try Roboto first (standard Android font)
        let candidates = self.load_family("Roboto", features)?;
        if !candidates.is_empty() {
            return Ok(candidates);
        }

        // Try DroidSans (older Android)
        let candidates = self.load_family("DroidSans", features)?;
        if !candidates.is_empty() {
            return Ok(candidates);
        }

        // Ultimate fallback: use first loaded font or load first from database
        if !self.loaded_fonts.is_empty() {
            return Ok(smallvec::smallvec![FontId(0)]);
        }

        // Get face_id first to avoid borrow conflict
        let face_id = self.font_system.db().faces().next().map(|f| f.id);
        if let Some(face_id) = face_id {
            if let Some(loaded_font) = self.font_system.get_font(face_id) {
                let font_id = FontId(self.loaded_fonts.len());
                self.loaded_fonts.push(LoadedFont {
                    font: loaded_font,
                    features: CosmicFontFeatures::default(),
                    is_known_emoji_font: false,
                });
                return Ok(smallvec::smallvec![font_id]);
            }
        }

        Ok(SmallVec::new())
    }

    /// Find best matching font from candidates based on style and weight.
    /// This is a simplified version of font-kit's find_best_match for Android.
    fn find_best_match(&self, candidates: &[FontId], font: &Font) -> usize {
        if candidates.is_empty() {
            return 0;
        }
        if candidates.len() == 1 {
            return 0;
        }

        let mut best_score = i32::MAX;
        let mut best_ix = 0;

        for (ix, &font_id) in candidates.iter().enumerate() {
            let loaded_font = &self.loaded_fonts[font_id.0];
            let face_id = loaded_font.font.id();
            let face_info = match self.font_system.db().face(face_id) {
                Some(info) => info,
                None => continue,
            };

            let mut score = 0i32;

            // Style matching (0 = exact, 1 = oblique/italic swap, 2 = mismatch)
            let style_score = match (font.style, face_info.style) {
                (FontStyle::Normal, cosmic_text::Style::Normal) => 0,
                (FontStyle::Italic, cosmic_text::Style::Italic) => 0,
                (FontStyle::Oblique, cosmic_text::Style::Oblique) => 0,
                (FontStyle::Italic, cosmic_text::Style::Oblique) => 1,
                (FontStyle::Oblique, cosmic_text::Style::Italic) => 1,
                _ => 2,
            };
            score += style_score * 1000;

            // Weight matching (difference in weight value)
            let weight_diff = (font.weight.0 as i32 - face_info.weight.0 as i32).abs();
            score += weight_diff;

            // Stretch matching (simplified - just prefer Normal)
            let stretch_score = match face_info.stretch {
                cosmic_text::Stretch::Normal => 0,
                _ => 100,
            };
            score += stretch_score;

            if score < best_score {
                best_score = score;
                best_ix = ix;
            }
        }

        best_ix
    }

    #[profiling::function]
    fn load_family(
        &mut self,
        name: &str,
        features: &FontFeatures,
    ) -> Result<SmallVec<[FontId; 4]>> {
        let name = crate::text_system::font_name_with_fallbacks(name, "Roboto");
        let name_lower = name.to_lowercase();

        // Search for fonts: exact match, case-insensitive, partial match, or sans-serif fallback
        let families: SmallVec<[_; 4]> = self
            .font_system
            .db()
            .faces()
            .filter(|face| {
                face.families.iter().any(|family| {
                    let family_lower = family.0.to_lowercase();
                    // Exact match
                    *name == family.0
                    // Case-insensitive match
                    || family_lower == name_lower
                    // Partial match
                    || family_lower.contains(&name_lower)
                    || name_lower.contains(&family_lower)
                })
            })
            .map(|face| (face.id, face.post_script_name.clone()))
            .collect();

        // For sans-serif requests, fall back to common Android fonts
        let families = if families.is_empty() && is_sans_serif_request(&name) {
            self.font_system
                .db()
                .faces()
                .filter(|face| {
                    face.families.iter().any(|family| {
                        let f = family.0.to_lowercase();
                        f.contains("roboto") || f.contains("droidsans") || f.contains("misans")
                    })
                })
                .map(|face| (face.id, face.post_script_name.clone()))
                .collect()
        } else {
            families
        };

        let mut loaded_font_ids = SmallVec::new();
        for (font_id, postscript_name) in families {
            let font = self
                .font_system
                .get_font(font_id)
                .context("Could not load font")?;

            // Skip fonts that can't render basic characters
            if font.as_swash().charmap().map('m') == 0 {
                self.font_system.db_mut().remove_face(font.id());
                continue;
            }

            let font_id = FontId(self.loaded_fonts.len());
            loaded_font_ids.push(font_id);
            self.loaded_fonts.push(LoadedFont {
                font,
                features: features.try_into()?,
                is_known_emoji_font: check_is_known_emoji_font(&postscript_name),
            });
        }

        Ok(loaded_font_ids)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        let glyph_metrics = self.loaded_font(font_id).font.as_swash().glyph_metrics(&[]);
        Ok(Size {
            width: glyph_metrics.advance_width(glyph_id.0 as u16),
            height: glyph_metrics.advance_height(glyph_id.0 as u16),
        })
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        let glyph_id = self.loaded_font(font_id).font.as_swash().charmap().map(ch);
        if glyph_id == 0 {
            None
        } else {
            Some(GlyphId(glyph_id.into()))
        }
    }

    fn raster_bounds(&mut self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        let image = self.render_glyph_image(params)?;
        Ok(Bounds {
            origin: point(image.placement.left.into(), (-image.placement.top).into()),
            size: size(image.placement.width.into(), image.placement.height.into()),
        })
    }

    #[profiling::function]
    fn rasterize_glyph(
        &mut self,
        params: &RenderGlyphParams,
        glyph_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        if glyph_bounds.size.width.0 == 0 || glyph_bounds.size.height.0 == 0 {
            anyhow::bail!("glyph bounds are empty");
        }

        let mut image = self.render_glyph_image(params)?;
        let bitmap_size = glyph_bounds.size;
        match image.content {
            swash::scale::image::Content::Color | swash::scale::image::Content::SubpixelMask => {
                // Convert from RGBA to BGRA.
                for pixel in image.data.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                Ok((bitmap_size, image.data))
            }
            swash::scale::image::Content::Mask => Ok((bitmap_size, image.data)),
        }
    }

    fn render_glyph_image(
        &mut self,
        params: &RenderGlyphParams,
    ) -> Result<swash::scale::image::Image> {
        let loaded_font = &self.loaded_fonts[params.font_id.0];
        let font_ref = loaded_font.font.as_swash();
        let pixel_size = params.font_size.0;

        let subpixel_offset = Vector::new(
            params.subpixel_variant.x as f32 / SUBPIXEL_VARIANTS_X as f32 / params.scale_factor,
            params.subpixel_variant.y as f32 / SUBPIXEL_VARIANTS_Y as f32 / params.scale_factor,
        );

        let mut scaler = self
            .swash_scale_context
            .builder(font_ref)
            .size(pixel_size)
            .hint(true)
            .build();

        let sources: &[Source] = if params.is_emoji {
            &[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ]
        } else {
            &[Source::Outline]
        };

        let mut renderer = Render::new(sources);
        renderer.transform(Some(Transform {
            xx: params.scale_factor,
            xy: 0.0,
            yx: 0.0,
            yy: params.scale_factor,
            x: 0.0,
            y: 0.0,
        }));

        if params.subpixel_rendering {
            // Subpixel rendering for LCD displays
            renderer
                .format(Format::subpixel_bgra())
                .offset(subpixel_offset);
        } else {
            renderer.format(Format::Alpha).offset(subpixel_offset);
        }

        let glyph_id: u16 = params.glyph_id.0.try_into()?;
        renderer
            .render(&mut scaler, glyph_id)
            .with_context(|| format!("unable to render glyph via swash for {params:?}"))
    }

    /// Handle fallback fonts chosen by cosmic_text during shaping.
    fn font_id_for_cosmic_id(&mut self, id: cosmic_text::fontdb::ID) -> Option<FontId> {
        // Check if already loaded
        if let Some(ix) = self
            .loaded_fonts
            .iter()
            .position(|f| f.font.id() == id)
        {
            return Some(FontId(ix));
        }

        // Load the font from cosmic_text
        let font = self.font_system.get_font(id)?;
        let face = self.font_system.db().face(id)?;

        let font_id = FontId(self.loaded_fonts.len());
        self.loaded_fonts.push(LoadedFont {
            font,
            features: CosmicFontFeatures::new(),
            is_known_emoji_font: check_is_known_emoji_font(&face.post_script_name),
        });

        Some(font_id)
    }

    #[profiling::function]
    fn layout_line(&mut self, text: &str, font_size: Pixels, font_runs: &[FontRun]) -> LineLayout {
        let mut attrs_list = AttrsList::new(&Attrs::new());
        let mut offs = 0;

        for run in font_runs {
            // Validate font_id and get font info
            let attrs = if run.font_id.0 < self.loaded_fonts.len() {
                let loaded_font = self.loaded_font(run.font_id);
                self.font_system
                    .db()
                    .face(loaded_font.font.id())
                    .and_then(|font| {
                        font.families.first().map(|family| {
                            Attrs::new()
                                .metadata(run.font_id.0)
                                .family(Family::Name(&family.0))
                                .stretch(font.stretch)
                                .style(font.style)
                                .weight(font.weight)
                                .font_features(loaded_font.features.clone())
                        })
                    })
            } else {
                None
            };

            attrs_list.add_span(offs..(offs + run.len), &attrs.unwrap_or_else(Attrs::new));
            offs += run.len;
        }

        let line = ShapeLine::new(
            &mut self.font_system,
            text,
            &attrs_list,
            cosmic_text::Shaping::Advanced,
            4,
        );
        let mut layout_lines = Vec::with_capacity(1);
        line.layout_to_buffer(
            &mut self.scratch,
            font_size.0,
            None,
            cosmic_text::Wrap::None,
            None,
            &mut layout_lines,
            None,
        );

        let Some(layout) = layout_lines.first() else {
            return LineLayout {
                font_size,
                width: 0.0.into(),
                ascent: 0.0.into(),
                descent: 0.0.into(),
                runs: Vec::new(),
                len: text.len(),
            };
        };

        let mut runs: Vec<ShapedRun> = Vec::new();
        for glyph in &layout.glyphs {
            // Resolve font_id, handling cosmic_text fallbacks
            let font_id = if glyph.metadata < self.loaded_fonts.len()
                && self.loaded_fonts[glyph.metadata].font.id() == glyph.font_id
            {
                FontId(glyph.metadata)
            } else {
                match self.font_id_for_cosmic_id(glyph.font_id) {
                    Some(id) => id,
                    None => continue,
                }
            };

            let is_emoji = self.loaded_font(font_id).is_known_emoji_font;

            // Skip variation selectors in emoji fonts
            if glyph.glyph_id == 3 && is_emoji {
                continue;
            }

            let shaped_glyph = ShapedGlyph {
                id: GlyphId(glyph.glyph_id as u32),
                position: point(glyph.x.into(), glyph.y.into()),
                index: glyph.start,
                is_emoji,
            };

            if let Some(last_run) = runs.last_mut().filter(|r| r.font_id == font_id) {
                last_run.glyphs.push(shaped_glyph);
            } else {
                runs.push(ShapedRun {
                    font_id,
                    glyphs: vec![shaped_glyph],
                });
            }
        }

        LineLayout {
            font_size,
            width: layout.w.into(),
            ascent: layout.max_ascent.into(),
            descent: layout.max_descent.into(),
            runs,
            len: text.len(),
        }
    }
}

impl TryFrom<&FontFeatures> for CosmicFontFeatures {
    type Error = anyhow::Error;

    fn try_from(features: &FontFeatures) -> Result<Self> {
        let mut result = CosmicFontFeatures::new();
        for feature in features.0.iter() {
            let name_bytes: [u8; 4] = feature
                .0
                .as_bytes()
                .try_into()
                .context("Incorrect feature flag format")?;

            let tag = cosmic_text::FeatureTag::new(&name_bytes);

            result.set(tag, feature.1);
        }
        Ok(result)
    }
}

impl From<FontWeight> for cosmic_text::Weight {
    fn from(value: FontWeight) -> Self {
        cosmic_text::Weight(value.0 as u16)
    }
}

impl From<FontStyle> for cosmic_text::Style {
    fn from(style: FontStyle) -> Self {
        match style {
            FontStyle::Normal => cosmic_text::Style::Normal,
            FontStyle::Italic => cosmic_text::Style::Italic,
            FontStyle::Oblique => cosmic_text::Style::Oblique,
        }
    }
}

fn check_is_known_emoji_font(postscript_name: &str) -> bool {
    postscript_name == "NotoColorEmoji"
        || postscript_name.contains("Emoji")
        || postscript_name.contains("emoji")
}

fn is_sans_serif_request(name: &str) -> bool {
    let name_lower = name.to_lowercase();
    name_lower.contains("sans")
        || name == ".ZedSans"
        || name == "Helvetica"
        || name == "Arial"
        || name == "IBM Plex Sans"
}
