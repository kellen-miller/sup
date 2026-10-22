use ratatui::style::{Color, Modifier, Style};

/// Streaming, output-only terminal decoding. No terminal actions are executed.
#[derive(Default)]
pub(super) struct OutputParser {
    // Disabling vte's std feature and OSC storage bounds discarded control strings.
    parser: vte::Parser<0>,
    state: OutputState,
}

#[derive(Default)]
struct OutputState {
    style: Style,
    carriage_return: bool,
}

struct OutputSink<'a, F> {
    state: &'a mut OutputState,
    write: F,
}

impl OutputParser {
    pub(super) fn parse(&mut self, input: &str, write: impl FnMut(char, Style)) {
        self.parser.advance(
            &mut OutputSink {
                state: &mut self.state,
                write,
            },
            input.as_bytes(),
        );
    }

    pub(super) fn clean(&mut self, input: &str) -> String {
        let mut output = String::new();
        self.parse(input, |character, _| {
            if character == '\r' {
                output.truncate(output.rfind('\n').map_or(0, |index| index + 1));
            } else {
                output.push(character);
            }
        });
        output
    }
}

impl<F: FnMut(char, Style)> vte::Perform for OutputSink<'_, F> {
    fn print(&mut self, character: char) {
        if character.is_control() {
            return;
        }

        if self.state.carriage_return {
            self.state.carriage_return = false;
            (self.write)('\r', self.state.style);
        }

        (self.write)(character, self.state.style);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => self.state.carriage_return = true,
            b'\n' => {
                self.state.carriage_return = false;
                (self.write)('\n', self.state.style);
            }
            b'\t' => self.print(' '),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], ignore: bool, action: char) {
        if action != 'm' || ignore || !intermediates.is_empty() {
            return;
        }

        let style = &mut self.state.style;
        let mut parameters = params.iter();
        while let Some(parameter) = parameters.next() {
            let code = parameter[0];
            match code {
                0 => *style = Style::default(),
                1 => *style = style.add_modifier(Modifier::BOLD),
                2 => *style = style.add_modifier(Modifier::DIM),
                3 => *style = style.add_modifier(Modifier::ITALIC),
                4 | 21 => *style = style.add_modifier(Modifier::UNDERLINED),
                5 => *style = style.add_modifier(Modifier::SLOW_BLINK),
                6 => *style = style.add_modifier(Modifier::RAPID_BLINK),
                7 => *style = style.add_modifier(Modifier::REVERSED),
                8 => *style = style.add_modifier(Modifier::HIDDEN),
                9 => *style = style.add_modifier(Modifier::CROSSED_OUT),
                22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
                23 => *style = style.remove_modifier(Modifier::ITALIC),
                24 => *style = style.remove_modifier(Modifier::UNDERLINED),
                25 => *style = style.remove_modifier(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK),
                27 => *style = style.remove_modifier(Modifier::REVERSED),
                28 => *style = style.remove_modifier(Modifier::HIDDEN),
                29 => *style = style.remove_modifier(Modifier::CROSSED_OUT),
                30..=37 | 90..=97 => style.fg = Some(basic_color(code)),
                40..=47 | 100..=107 => style.bg = Some(basic_color(code - 10)),
                38 => style.fg = extended_color(parameter, &mut parameters).or(style.fg),
                48 => style.bg = extended_color(parameter, &mut parameters).or(style.bg),
                39 => style.fg = None,
                49 => style.bg = None,
                _ => {}
            }
        }
    }
}

fn basic_color(code: u16) -> Color {
    const COLORS: [Color; 16] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    let index = if code >= 90 { code - 90 + 8 } else { code - 30 };
    COLORS[index as usize]
}

fn extended_color(parameter: &[u16], parameters: &mut vte::ParamsIter<'_>) -> Option<Color> {
    let mut channels = [0; 5];
    let values = if parameter.len() > 1 {
        &parameter[1..]
    } else {
        channels[0] = *parameters.next()?.first()?;
        let count = match channels[0] {
            5 => 2,
            2 => 4,
            _ => return None,
        };
        for channel in &mut channels[1..count] {
            *channel = *parameters.next()?.first()?;
        }

        &channels[..count]
    };
    match values {
        [5, index] => Some(Color::Indexed(u8::try_from(*index).ok()?)),
        [2, red, green, blue] | [2, 0, red, green, blue] => Some(Color::Rgb(
            u8::try_from(*red).ok()?,
            u8::try_from(*green).ok()?,
            u8::try_from(*blue).ok()?,
        )),
        _ => None,
    }
}
