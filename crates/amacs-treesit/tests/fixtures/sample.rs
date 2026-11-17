use std::collections::{
    HashMap,
    HashSet,
};

/// A point in the plane.
#[derive(Debug, Clone)]
pub struct Point {
    x: f64,
    y: f64,
}

const LIMIT: usize = 10;

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn norm(&self) -> f64
    where
        Self: Sized,
    {
        let sum = self.x * self.x
            + self.y * self.y;
        sum.sqrt()
    }
}

fn main() {
    let points: Vec<Point> = (0..LIMIT)
        .map(|i| Point::new(i as f64, 1.0))
        .filter(|p| {
            p.norm() > 2.0
        })
        .collect();
    for point in &points {
        match point.x as i32 {
            0 => println!("zero"),
            n if n > 5 => {
                println!("big {n}");
            }
            _ => {}
        }
    }
    let text = describe(
        "a string\n",
        'c',
    );
    // The end.
}
