//! Freeverb-style mono reverb (8 parallel damped combs into 4 series allpasses).

const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const FIXED_GAIN: f32 = 0.015;
const SCALE_ROOM: f32 = 0.28;
const OFFSET_ROOM: f32 = 0.7;
const SCALE_DAMP: f32 = 0.4;

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    filter_store: f32,
    feedback: f32,
    damp1: f32,
    damp2: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            idx: 0,
            filter_store: 0.0,
            feedback: 0.5,
            damp1: 0.5,
            damp2: 0.5,
        }
    }

    #[inline]
    fn tick(&mut self, input: f32) -> f32 {
        let output = self.buf[self.idx];
        self.filter_store = output * self.damp2 + self.filter_store * self.damp1;
        self.buf[self.idx] = input + self.filter_store * self.feedback;
        self.idx = (self.idx + 1) % self.buf.len();
        output
    }
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            idx: 0,
        }
    }

    #[inline]
    fn tick(&mut self, input: f32) -> f32 {
        let bufout = self.buf[self.idx];
        let output = -input + bufout;
        self.buf[self.idx] = input + bufout * 0.5;
        self.idx = (self.idx + 1) % self.buf.len();
        output
    }
}

pub struct Reverb {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
    mix: f32,
    size: f32,
    damp: f32,
}

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let scale = sample_rate / 44_100.0;
        let mut r = Self {
            combs: COMB_TUNING
                .iter()
                .map(|&t| Comb::new((t as f32 * scale) as usize))
                .collect(),
            allpasses: ALLPASS_TUNING
                .iter()
                .map(|&t| Allpass::new((t as f32 * scale) as usize))
                .collect(),
            mix: 0.0,
            size: -1.0,
            damp: -1.0,
        };
        r.set(0.6, 0.5, 0.0);
        r
    }

    pub fn set(&mut self, size: f32, damp: f32, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
        let size = size.clamp(0.0, 1.0);
        let damp = damp.clamp(0.0, 1.0);
        if (size - self.size).abs() > 1e-4 || (damp - self.damp).abs() > 1e-4 {
            self.size = size;
            self.damp = damp;
            let feedback = size * SCALE_ROOM + OFFSET_ROOM;
            let damp1 = damp * SCALE_DAMP;
            for c in &mut self.combs {
                c.feedback = feedback;
                c.damp1 = damp1;
                c.damp2 = 1.0 - damp1;
            }
        }
    }

    pub fn reset(&mut self) {
        for c in &mut self.combs {
            c.buf.fill(0.0);
            c.filter_store = 0.0;
        }
        for a in &mut self.allpasses {
            a.buf.fill(0.0);
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        if self.mix <= 0.0 {
            return;
        }
        for x in buf.iter_mut() {
            let input = *x * FIXED_GAIN;
            let mut out = 0.0;
            for c in &mut self.combs {
                out += c.tick(input);
            }
            for a in &mut self.allpasses {
                out = a.tick(out);
            }
            *x = *x * (1.0 - self.mix) + out * self.mix;
        }
    }
}
