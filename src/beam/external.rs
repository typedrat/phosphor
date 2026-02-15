use nom::IResult;
use nom::Parser;
use nom::bytes::complete::tag;
use nom::character::complete::{char, multispace0, space1};
use nom::number::complete::float;
use nom::sequence::preceded;

use super::{BeamSample, BeamSource, BeamState};

/// Minimum subdivisions per segment.
const MIN_SUBDIVISIONS: usize = 2;

/// A parsed command from the external protocol.
pub enum Command {
    /// A single beam sample: `B x y intensity dt`
    Beam(BeamSample),
    /// A line segment: `L x0 y0 x1 y1 intensity`
    Segment {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        intensity: f32,
    },
    /// Frame sync: `F`
    FrameSync,
}

fn sp_float(input: &str) -> IResult<&str, f32> {
    preceded(space1, float).parse(input)
}

fn parse_beam(input: &str) -> IResult<&str, Command> {
    let (rest, (_, x, y, intensity, dt)) =
        (char('B'), sp_float, sp_float, sp_float, sp_float).parse(input)?;
    Ok((
        rest,
        Command::Beam(BeamSample {
            x,
            y,
            intensity,
            dt,
        }),
    ))
}

fn parse_segment(input: &str) -> IResult<&str, Command> {
    let (rest, (_, x0, y0, x1, y1, intensity)) =
        (char('L'), sp_float, sp_float, sp_float, sp_float, sp_float).parse(input)?;
    Ok((
        rest,
        Command::Segment {
            x0,
            y0,
            x1,
            y1,
            intensity,
        },
    ))
}

fn parse_frame_sync(input: &str) -> IResult<&str, Command> {
    let (rest, _) = tag("F").parse(input)?;
    Ok((rest, Command::FrameSync))
}

/// Parse a single line of the external protocol.
///
/// Protocol:
/// - `B x y intensity dt` — a single beam sample
/// - `L x0 y0 x1 y1 intensity` — a line segment
/// - `F` — frame sync
/// - `#...` — comment (returns None)
/// - empty/whitespace — ignored (returns None)
pub fn parse_line(line: &str) -> anyhow::Result<Option<Command>> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }

    // Skip leading whitespace, then try each command parser
    let input = multispace0::<&str, nom::error::Error<&str>>
        .parse(trimmed)
        .map(|(rest, _)| rest)
        .unwrap_or(trimmed);

    if let Ok((_, cmd)) = parse_beam(input) {
        return Ok(Some(cmd));
    }
    if let Ok((_, cmd)) = parse_segment(input) {
        return Ok(Some(cmd));
    }
    if let Ok((_, cmd)) = parse_frame_sync(input) {
        return Ok(Some(cmd));
    }

    anyhow::bail!("unknown command: {trimmed}");
}

/// Subdivide a segment into beam samples, spaced within one spot radius.
fn subdivide_segment(
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    intensity: f32,
    beam_speed: f32,
    beam: &BeamState,
) -> Vec<BeamSample> {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let length = (dx * dx + dy * dy).sqrt();
    let steps = ((length / beam.spot_radius).ceil() as usize).max(MIN_SUBDIVISIONS);
    let dt = length / (beam_speed * steps as f32);

    (0..steps)
        .map(|i| {
            let t = (i as f32 + 0.5) / steps as f32;
            BeamSample {
                x: x0 + dx * t,
                y: y0 + dy * t,
                intensity,
                dt,
            }
        })
        .collect()
}

pub struct ExternalSource {
    pub beam_speed: f32,
    lines: Vec<String>,
    position: usize,
}

impl ExternalSource {
    pub fn new(beam_speed: f32) -> Self {
        Self {
            beam_speed,
            lines: Vec::new(),
            position: 0,
        }
    }

    /// Feed lines from the external protocol into the source.
    pub fn push_lines(&mut self, lines: impl IntoIterator<Item = String>) {
        self.lines.extend(lines);
    }
}

impl BeamSource for ExternalSource {
    fn generate(&mut self, _count: usize, beam: &BeamState) -> Vec<BeamSample> {
        let mut out = Vec::new();

        while self.position < self.lines.len() {
            let line = &self.lines[self.position];
            self.position += 1;

            match parse_line(line) {
                Ok(Some(Command::Beam(sample))) => out.push(sample),
                Ok(Some(Command::Segment {
                    x0,
                    y0,
                    x1,
                    y1,
                    intensity,
                })) => {
                    out.extend(subdivide_segment(
                        x0,
                        y0,
                        x1,
                        y1,
                        intensity,
                        self.beam_speed,
                        beam,
                    ));
                }
                Ok(Some(Command::FrameSync)) => break,
                Ok(None) | Err(_) => {}
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BEAM: BeamState = BeamState { spot_radius: 0.001 };

    #[test]
    fn parse_beam_command() {
        let cmd = parse_line("B 0.5 0.75 1.0 0.001").unwrap().unwrap();
        match cmd {
            Command::Beam(s) => {
                assert!((s.x - 0.5).abs() < f32::EPSILON);
                assert!((s.y - 0.75).abs() < f32::EPSILON);
                assert!((s.intensity - 1.0).abs() < f32::EPSILON);
                assert!((s.dt - 0.001).abs() < f32::EPSILON);
            }
            _ => panic!("expected Beam command"),
        }
    }

    #[test]
    fn parse_comment_returns_none() {
        assert!(parse_line("# this is a comment").unwrap().is_none());
    }

    #[test]
    fn parse_empty_line_returns_none() {
        assert!(parse_line("").unwrap().is_none());
        assert!(parse_line("   ").unwrap().is_none());
    }

    #[test]
    fn parse_frame_sync() {
        let cmd = parse_line("F").unwrap().unwrap();
        assert!(matches!(cmd, Command::FrameSync));
    }

    #[test]
    fn parse_invalid_returns_error() {
        assert!(parse_line("X garbage").is_err());
        assert!(parse_line("B only_two 0.5").is_err());
    }

    #[test]
    fn parse_segment_command() {
        let cmd = parse_line("L 0.0 0.0 1.0 0.0 1.0").unwrap().unwrap();
        assert!(matches!(cmd, Command::Segment { .. }));
    }

    #[test]
    fn generate_processes_beam_lines() {
        let mut src = ExternalSource::new(1.0);
        src.push_lines(vec![
            "B 0.5 0.75 1.0 0.001".into(),
            "B 0.25 0.25 0.5 0.002".into(),
        ]);
        let samples = src.generate(0, &TEST_BEAM);
        assert_eq!(samples.len(), 2);
        assert!((samples[0].x - 0.5).abs() < f32::EPSILON);
        assert!((samples[1].x - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn generate_subdivides_segments() {
        let mut src = ExternalSource::new(1.0);
        src.push_lines(vec!["L 0.0 0.0 1.0 0.0 1.0".into()]);
        let samples = src.generate(0, &TEST_BEAM);
        assert!(!samples.is_empty());
        for s in &samples {
            assert!((s.y).abs() < 0.01);
        }
    }

    #[test]
    fn generate_stops_at_frame_sync() {
        let mut src = ExternalSource::new(1.0);
        src.push_lines(vec![
            "B 0.1 0.1 1.0 0.001".into(),
            "F".into(),
            "B 0.9 0.9 1.0 0.001".into(),
        ]);
        let first_frame = src.generate(0, &TEST_BEAM);
        assert_eq!(first_frame.len(), 1);
        assert!((first_frame[0].x - 0.1).abs() < f32::EPSILON);

        let second_frame = src.generate(0, &TEST_BEAM);
        assert_eq!(second_frame.len(), 1);
        assert!((second_frame[0].x - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn generate_skips_comments_and_blanks() {
        let mut src = ExternalSource::new(1.0);
        src.push_lines(vec![
            "# header comment".into(),
            "".into(),
            "B 0.5 0.5 1.0 0.001".into(),
            "   ".into(),
        ]);
        let samples = src.generate(0, &TEST_BEAM);
        assert_eq!(samples.len(), 1);
    }
}
