use crate::bitmap;
use crate::dictionary::{Character, CharacterId, Dictionary};
use crate::export::js;
use crate::shape::{Line, Shape};
use crate::timeline::{self, Frame, Timeline, TimelineBuilder};
use image::GenericImageView;
use svg::node::element::{
    path, ClipPath, Definitions, Group, Image, LinearGradient, Path, Pattern, RadialGradient,
    Rectangle, Script, Stop,
};
use swf_tree as swf;

// FIXME(eddyb) upstream these as methods on `swf-fixed` types.
fn ufixed8p8_epsilons(x: &swf::fixed_point::Ufixed8P8) -> u16 {
    unsafe { std::mem::transmute_copy(x) }
}
fn ufixed8p8_to_f64(x: &swf::fixed_point::Ufixed8P8) -> f64 {
    ufixed8p8_epsilons(x) as f64 / (1 << 8) as f64
}

mod animate;

#[derive(Default)]
pub struct Config {
    pub use_js: bool,
}

pub fn export(movie: &swf::Movie, config: Config) -> svg::Document {
    let mut dictionary = Dictionary::default();

    let mut bg = [0, 0, 0];
    let mut timeline_builder = TimelineBuilder::default();
    for tag in &movie.tags {
        match tag {
            swf::Tag::SetBackgroundColor(set_bg) => {
                let c = &set_bg.color;
                bg = [c.r, c.g, c.b];
            }
            swf::Tag::DefineShape(def) => {
                dictionary.define(CharacterId(def.id), Character::Shape(Shape::from(def)))
            }
            // FIXME(eddyb) deduplicate this.
            swf::Tag::DefineSprite(def) => {
                let mut timeline_builder = TimelineBuilder::default();
                for tag in &def.tags {
                    match tag {
                        swf::Tag::PlaceObject(place) => timeline_builder.place_object(place),
                        swf::Tag::RemoveObject(remove) => timeline_builder.remove_object(remove),
                        swf::Tag::DoAction(do_action) => timeline_builder.do_action(do_action),
                        swf::Tag::ShowFrame => timeline_builder.advance_frame(),
                        swf::Tag::Unknown(tag) => {
                            if let Some(label) = timeline::FrameLabel::try_parse(tag) {
                                timeline_builder.frame_label(label)
                            } else {
                                eprintln!("unknown sprite tag: {:?}", tag);
                            }
                        }
                        _ => eprintln!("unknown sprite tag: {:?}", tag),
                    }
                }
                let timeline = timeline_builder.finish(Frame(def.frame_count as u16));
                dictionary.define(CharacterId(def.id), Character::Sprite(timeline))
            }
            swf::Tag::DefineDynamicText(def) => {
                dictionary.define(CharacterId(def.id), Character::DynamicText(def))
            }
            swf::Tag::PlaceObject(place) => timeline_builder.place_object(place),
            swf::Tag::RemoveObject(remove) => timeline_builder.remove_object(remove),
            swf::Tag::DoAction(do_action) => timeline_builder.do_action(do_action),
            swf::Tag::ShowFrame => timeline_builder.advance_frame(),
            swf::Tag::Unknown(tag) => {
                if let Some(def) = bitmap::DefineBitmap::try_parse(tag) {
                    dictionary.define(def.id, Character::Bitmap(def.image));
                } else if let Some(label) = timeline::FrameLabel::try_parse(tag) {
                    timeline_builder.frame_label(label)
                } else {
                    eprintln!("unknown tag: {:?}", tag);
                }
            }
            _ => eprintln!("unknown tag: {:?}", tag),
        }
    }
    let timeline = timeline_builder.finish(Frame(movie.header.frame_count));

    let view_box = {
        let r = &movie.header.frame_size;
        (r.x_min, r.y_min, r.x_max - r.x_min, r.y_max - r.y_min)
    };

    let mut cx = Context {
        config,
        frame_rate: ufixed8p8_to_f64(&movie.header.frame_rate),

        svg_defs: Definitions::new(),
        next_gradient_id: 0,
    };

    let mut js_sprites = js::code! {};
    for (&id, character) in &dictionary.characters {
        let svg_character = cx.export_character(id, character);
        cx.add_svg_def(svg_character.set("id", format!("c_{}", id.0)));

        if cx.config.use_js {
            if let Character::Sprite(timeline) = character {
                js_sprites += js::code! {
                    "sprites[", id.0, "] = ", js::timeline::export(timeline), ";\n"
                };
            }
        }
    }

    let mut svg_document = svg::Document::new()
        .set("viewBox", view_box)
        .set("style", "background: black")
        .add(
            Rectangle::new()
                .set("width", "100%")
                .set("height", "100%")
                .set("fill", format!("#{:02x}{:02x}{:02x}", bg[0], bg[1], bg[2])),
        );

    cx.add_svg_def(
        ClipPath::new().set("id", "viewBox_clip").add(
            Rectangle::new()
                .set("x", view_box.0)
                .set("y", view_box.1)
                .set("width", view_box.2)
                .set("height", view_box.3),
        ),
    );

    if !cx.config.use_js {
        let svg_body = cx
            .export_timeline(None, &timeline)
            .set("clip-path", "url(#viewBox_clip)");
        svg_document = svg_document.add(cx.svg_defs).add(svg_body);
    } else {
        svg_document = svg_document
            .add(cx.svg_defs)
            .add(
                Group::new()
                    .set("id", "body")
                    .set("clip-path", "url(#viewBox_clip)"),
            )
            .add(
                js::code! {
                    "var timeline = ", js::timeline::export(&timeline), ";\n",
                    "var sprites = [];\n",
                    js_sprites,
                    "var frame_rate = ", cx.frame_rate, ";\n\n",
                    include_str!("runtime.js")
                }
                .to_svg(),
            );
    }

    svg_document
}

impl js::Code {
    fn to_svg(self) -> Script {
        Script::new(
            js::code! {
                "// <![CDATA[\n",
                self, "\n",
                "// ]]>\n"
            }
            .0,
        )
    }
}

struct Context {
    config: Config,
    frame_rate: f64,

    svg_defs: Definitions,
    next_gradient_id: usize,
}

impl Context {
    fn add_svg_def(&mut self, node: impl svg::Node) {
        self.svg_defs = std::mem::replace(&mut self.svg_defs, Definitions::new()).add(node);
    }

    fn rgba_to_svg(&self, c: &swf::StraightSRgba8) -> String {
        if c.a == 0xff {
            format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
        } else {
            format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, c.a)
        }
    }

    fn fill_to_svg(&mut self, style: &swf::FillStyle) -> String {
        match style {
            swf::FillStyle::Solid(solid) => self.rgba_to_svg(&solid.color),
            // FIXME(eddyb) don't ignore the gradient transformation matrix.
            // TODO(eddyb) cache identical gradients.
            swf::FillStyle::LinearGradient(gradient) => {
                let mut svg_gradient = LinearGradient::new();
                for stop in &gradient.gradient.colors {
                    svg_gradient = svg_gradient.add(
                        Stop::new()
                            .set(
                                "offset",
                                format!("{}%", (stop.ratio as f64 / 255.0) * 100.0),
                            )
                            .set("stop-color", self.rgba_to_svg(&stop.color)),
                    );
                }

                let id = self.next_gradient_id;
                self.next_gradient_id += 1;

                self.add_svg_def(svg_gradient.set("id", format!("grad_{}", id)));

                format!("url(#grad_{})", id)
            }
            swf::FillStyle::RadialGradient(gradient) => {
                // FIXME(eddyb) remove duplication between linear and radial gradients.
                let mut svg_gradient = RadialGradient::new();
                for stop in &gradient.gradient.colors {
                    svg_gradient = svg_gradient.add(
                        Stop::new()
                            .set(
                                "offset",
                                format!("{}%", (stop.ratio as f64 / 255.0) * 100.0),
                            )
                            .set("stop-color", self.rgba_to_svg(&stop.color)),
                    );
                }

                let id = self.next_gradient_id;
                self.next_gradient_id += 1;

                self.add_svg_def(svg_gradient.set("id", format!("grad_{}", id)));

                format!("url(#grad_{})", id)
            }
            // FIXME(eddyb) don't ignore the bitmap transformation matrix,
            // and the `repeating` and `smoothed` options.
            swf::FillStyle::Bitmap(bitmap) => format!("url(#pat_{})", bitmap.bitmap_id),
            _ => {
                eprintln!("unsupported fill: {:?}", style);
                // TODO(eddyb) implement focal gradient support.
                "#ff00ff".to_string()
            }
        }
    }

    fn export_character(&mut self, id: CharacterId, character: &Character) -> Group {
        let mut g = Group::new();
        match character {
            Character::Shape(shape) => {
                // TODO(eddyb) do the transforms need to take `shape.center` into account?

                let path_data = |path: &[Line]| {
                    let start = path.first()?.from;

                    let mut data = path::Data::new().move_to(start.x_y());
                    let mut pos = start;

                    for line in path {
                        if line.from != pos {
                            data = data.move_to(line.from.x_y());
                        }

                        if let Some(control) = line.bezier_control {
                            data = data
                                .quadratic_curve_to((control.x, control.y, line.to.x, line.to.y));
                        } else {
                            data = data.line_to(line.to.x_y());
                        }

                        pos = line.to;
                    }

                    Some((start, data, pos))
                };

                for fill in &shape.fill {
                    if let Some((start, mut data, end)) = path_data(&fill.path) {
                        if start == end {
                            data = data.close();
                        }

                        g = g.add(
                            Path::new()
                                .set("fill", self.fill_to_svg(fill.style))
                                // TODO(eddyb) confirm/infirm the correctness of this.
                                .set("fill-rule", "evenodd")
                                .set("d", data),
                        );
                    }
                }

                for stroke in &shape.stroke {
                    if let Some((start, mut data, end)) = path_data(&stroke.path) {
                        if !stroke.style.no_close && start == end {
                            data = data.close();
                        }

                        // TODO(eddyb) implement cap/join support.

                        g = g.add(
                            Path::new()
                                .set("fill", "none")
                                .set("stroke", self.fill_to_svg(&stroke.style.fill))
                                .set("stroke-width", stroke.style.width)
                                .set("d", data),
                        );
                    }
                }

                g
            }

            Character::Bitmap(image) => {
                let mut data_url = "data:image/png;base64,".to_string();
                {
                    let mut png = vec![];
                    image.write_to(&mut png, image::PNG).unwrap();
                    base64::encode_config_buf(&png, base64::STANDARD, &mut data_url);
                }
                g.add(
                    Pattern::new()
                        .set("id", format!("pat_{}", id.0))
                        .set("width", 1)
                        .set("height", 1)
                        .add(
                            Image::new()
                                .set("href", data_url)
                                .set("width", image.width() * 20)
                                .set("height", image.height() * 20),
                        ),
                )
            }

            // TODO(eddyb) figure out if there's anything to be done here
            // wrt synchronizing the animiation timelines of sprites.
            Character::Sprite(timeline) => self.export_timeline(Some(id), timeline),

            Character::DynamicText(def) => {
                let mut text = svg::node::element::Text::new().add(svg::node::Text::new(
                    def.text.as_ref().map_or("", |s| &s[..]),
                ));

                if let Some(size) = def.font_size {
                    text = text.set("font-size", size);
                }

                if let Some(color) = &def.color {
                    text = text.set("fill", self.rgba_to_svg(color));
                }

                g.add(text)
            }
        }
    }

    fn export_timeline(&self, id: Option<CharacterId>, timeline: &Timeline) -> Group {
        let frame_duration = 1.0 / self.frame_rate;
        let movie_duration = timeline.frame_count.0 as f64 * frame_duration;

        let mut g = Group::new();
        if self.config.use_js {
            return g;
        }
        let id_prefix = id.map_or(String::new(), |id| format!("c_{}_", id.0));
        for (&depth, layer) in &timeline.layers {
            let id_prefix = format!("{}d_{}_", id_prefix, depth.0);
            let mut animation =
                animate::ObjectAnimation::new(id_prefix, timeline.frame_count, movie_duration);
            for (&frame, obj) in &layer.frames {
                animation.add(frame, obj.as_ref());
            }
            g = g.add(animation.to_svg());
        }
        g
    }
}
