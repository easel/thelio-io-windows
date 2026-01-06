// From https://github.com/pop-os/system76-power/blob/master/src/fan.rs
//TODO: use a shared crate

use std::collections::VecDeque;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FanPoint {
    // Temperature in hundredths of a degree, 10000 = 100C
    temp: i16,
    // duty in hundredths of a percent, 10000 = 100%
    duty: u16,
}

impl FanPoint {
    pub fn new(temp: i16, duty: u16) -> Self { Self { temp, duty } }

    /// Find the duty between two points and a given temperature, if the temperature
    /// lies within this range.
    fn get_duty_between_points(self, next: FanPoint, temp: i16) -> Option<u16> {
        // If the temp matches the next point, return the next point duty
        if temp == next.temp {
            return Some(next.duty);
        }

        // If the temp matches the previous point, return the previous point duty
        if temp == self.temp {
            return Some(self.duty);
        }

        // If the temp is in between the previous and next points, interpolate the duty
        if self.temp < temp && next.temp > temp {
            return Some(self.interpolate_duties(next, temp));
        }

        None
    }

    /// Interpolates the current duty with that of the given next point and temperature.
    fn interpolate_duties(self, next: FanPoint, temp: i16) -> u16 {
        let dtemp = next.temp - self.temp;
        let dduty = next.duty - self.duty;

        let slope = f32::from(dduty) / f32::from(dtemp);

        let temp_offset = temp - self.temp;
        let duty_offset = (slope * f32::from(temp_offset)).round();

        self.duty + (duty_offset as u16)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FanCurve {
    points: Vec<FanPoint>,
}

impl FanCurve {
    /// Adds a point to the fan curve
    pub fn append(mut self, temp: i16, duty: u16) -> Self {
        self.points.push(FanPoint::new(temp, duty));
        self
    }

    /// The standard fan curve
    pub fn standard() -> Self {
        Self::default()
            .append(44_99, 0_00)
            .append(45_00, 30_00)
            .append(55_00, 35_00)
            .append(65_00, 40_00)
            .append(75_00, 50_00)
            .append(78_00, 60_00)
            .append(81_00, 70_00)
            .append(84_00, 80_00)
            .append(86_00, 90_00)
            .append(88_00, 100_00)
    }

    /// Fan curve for threadripper 2
    pub fn threadripper2() -> Self {
        Self::default()
            .append(00_00, 30_00)
            .append(40_00, 40_00)
            .append(47_50, 50_00)
            .append(55_00, 65_00)
            .append(62_50, 85_00)
            .append(66_25, 100_00)
    }

    /// Fan curve for HEDT systems
    pub fn hedt() -> Self {
        Self::default()
            .append(00_00, 30_00)
            .append(50_00, 35_00)
            .append(60_00, 45_00)
            .append(70_00, 55_00)
            .append(74_00, 60_00)
            .append(76_00, 70_00)
            .append(78_00, 80_00)
            .append(81_00, 100_00)
    }

    /// Fan curve for xeon
    pub fn xeon() -> Self {
        Self::default()
            .append(00_00, 40_00)
            .append(50_00, 40_00)
            .append(55_00, 45_00)
            .append(60_00, 50_00)
            .append(65_00, 55_00)
            .append(70_00, 60_00)
            .append(72_00, 65_00)
            .append(74_00, 80_00)
            .append(76_00, 85_00)
            .append(77_00, 90_00)
            .append(78_00, 100_00)
    }

    /// Quiet fan curve - keeps fans off longer, ramps up more gradually
    pub fn quiet() -> Self {
        Self::default()
            .append(54_99, 0_00)    // Fans off until 55°C (was 45°C)
            .append(55_00, 25_00)   // Start at 25% (was 30%)
            .append(65_00, 30_00)   // Gradual ramp
            .append(75_00, 40_00)   // 40% at 75°C (was 50%)
            .append(80_00, 50_00)   // 50% at 80°C
            .append(83_00, 60_00)   // 60% at 83°C
            .append(86_00, 70_00)   // 70% at 86°C
            .append(89_00, 80_00)   // 80% at 89°C
            .append(92_00, 90_00)   // 90% at 92°C
            .append(95_00, 100_00)  // 100% only at 95°C
    }

    pub fn get_duty(&self, temp: i16) -> Option<u16> {
        // If the temp is less than the first point, return the first point duty
        if let Some(first) = self.points.first() {
            if temp < first.temp {
                return Some(first.duty);
            }
        }

        // Use when we upgrade to 1.28.0
        // for &[prev, next] in self.points.windows(2) {

        for window in self.points.windows(2) {
            let prev = window[0];
            let next = window[1];
            if let Some(duty) = prev.get_duty_between_points(next, temp) {
                return Some(duty);
            }
        }

        // If the temp is greater than the last point, return the last point duty
        if let Some(last) = self.points.last() {
            if temp > last.temp {
                return Some(last.duty);
            }
        }

        // If there are no points, return None
        None
    }
}

/// Fan controller with temperature smoothing and hysteresis.
///
/// This prevents "fan panic" from short temperature spikes common on modern CPUs
/// (especially AMD Zen 3) by:
/// - Averaging temperature over a rolling window
/// - Delaying fan speed increases (ramp-up delay)
/// - Delaying fan speed decreases even more (ramp-down delay)
pub struct FanController {
    curve: FanCurve,
    /// Rolling window of temperature readings (in hundredths of a degree)
    temp_history: VecDeque<i16>,
    /// How many samples to average (e.g., 5 = 5 seconds at 1 sample/sec)
    smoothing_window: usize,
    /// Current fan duty (hundredths of a percent)
    current_duty: u16,
    /// When we last increased fan speed
    last_increase: Option<Instant>,
    /// When we last decreased fan speed
    last_decrease: Option<Instant>,
    /// Seconds to wait before ramping up
    ramp_up_delay_secs: f32,
    /// Seconds to wait before ramping down
    ramp_down_delay_secs: f32,
    /// Minimum duty change to act on (prevents micro-adjustments)
    min_duty_change: u16,
}

impl FanController {
    /// Create a new fan controller with default settings optimized for Zen 3.
    ///
    /// Defaults:
    /// - 5 second temperature smoothing window
    /// - 3 second ramp-up delay
    /// - 10 second ramp-down delay
    /// - 200 (2%) minimum duty change
    pub fn new(curve: FanCurve) -> Self {
        Self {
            curve,
            temp_history: VecDeque::with_capacity(10),
            smoothing_window: 5,
            current_duty: 0,
            last_increase: None,
            last_decrease: None,
            ramp_up_delay_secs: 3.0,
            ramp_down_delay_secs: 10.0,
            min_duty_change: 2_00, // 2%
        }
    }

    /// Configure the smoothing window size (in samples, typically seconds)
    pub fn with_smoothing_window(mut self, samples: usize) -> Self {
        self.smoothing_window = samples.max(1);
        self.temp_history = VecDeque::with_capacity(samples + 5);
        self
    }

    /// Configure ramp-up delay in seconds
    pub fn with_ramp_up_delay(mut self, secs: f32) -> Self {
        self.ramp_up_delay_secs = secs.max(0.0);
        self
    }

    /// Configure ramp-down delay in seconds
    pub fn with_ramp_down_delay(mut self, secs: f32) -> Self {
        self.ramp_down_delay_secs = secs.max(0.0);
        self
    }

    /// Configure minimum duty change threshold (in hundredths of a percent)
    pub fn with_min_duty_change(mut self, duty: u16) -> Self {
        self.min_duty_change = duty;
        self
    }

    /// Get the smoothed (averaged) temperature
    fn smoothed_temp(&self) -> i16 {
        if self.temp_history.is_empty() {
            return 0;
        }
        let sum: i32 = self.temp_history.iter().map(|&t| t as i32).sum();
        (sum / self.temp_history.len() as i32) as i16
    }

    /// Update the controller with a new temperature reading.
    /// Returns the duty cycle to set (if it should change) and debug info.
    pub fn update(&mut self, temp: i16) -> FanControllerOutput {
        let now = Instant::now();

        // Add to history, maintain window size
        self.temp_history.push_back(temp);
        while self.temp_history.len() > self.smoothing_window {
            self.temp_history.pop_front();
        }

        let smoothed_temp = self.smoothed_temp();
        let target_duty = self.curve.get_duty(smoothed_temp).unwrap_or(0);

        let mut output = FanControllerOutput {
            instant_temp: temp,
            smoothed_temp,
            target_duty,
            actual_duty: self.current_duty,
            duty_changed: false,
            reason: "holding",
        };

        // Calculate duty difference
        let duty_diff = (target_duty as i32 - self.current_duty as i32).abs() as u16;

        // Ignore changes smaller than threshold (unless it's a big jump for safety)
        if duty_diff < self.min_duty_change && target_duty < 100_00 {
            return output;
        }

        if target_duty > self.current_duty {
            // Want to increase fan speed
            let should_increase = match self.last_increase {
                None => {
                    // First increase request - start the timer
                    self.last_increase = Some(now);
                    // But allow immediate increase if it's a large jump (safety)
                    duty_diff >= 10_00 // 10% jump = immediate
                }
                Some(last) => {
                    // Check if we've waited long enough
                    now.duration_since(last).as_secs_f32() >= self.ramp_up_delay_secs
                }
            };

            if should_increase {
                self.current_duty = target_duty;
                self.last_increase = Some(now);
                self.last_decrease = None;
                output.actual_duty = target_duty;
                output.duty_changed = true;
                output.reason = "ramp-up";
            } else {
                output.reason = "ramp-up delayed";
            }
        } else if target_duty < self.current_duty {
            // Want to decrease fan speed
            let should_decrease = match self.last_decrease {
                None => {
                    // First decrease request - start the timer
                    self.last_decrease = Some(now);
                    false
                }
                Some(last) => {
                    // Check if we've waited long enough
                    now.duration_since(last).as_secs_f32() >= self.ramp_down_delay_secs
                }
            };

            if should_decrease {
                self.current_duty = target_duty;
                self.last_decrease = Some(now);
                self.last_increase = None;
                output.actual_duty = target_duty;
                output.duty_changed = true;
                output.reason = "ramp-down";
            } else {
                output.reason = "ramp-down delayed";
            }
        } else {
            // No change needed
            self.last_increase = None;
            self.last_decrease = None;
            output.reason = "steady";
        }

        output
    }

    /// Get the current duty setting
    pub fn current_duty(&self) -> u16 {
        self.current_duty
    }

    /// Force-set the duty (for initialization)
    pub fn set_duty(&mut self, duty: u16) {
        self.current_duty = duty;
    }
}

/// Output from the fan controller update
#[derive(Debug)]
pub struct FanControllerOutput {
    /// Instantaneous temperature reading (hundredths of a degree)
    pub instant_temp: i16,
    /// Smoothed/averaged temperature (hundredths of a degree)
    pub smoothed_temp: i16,
    /// What duty the curve says we should be at
    pub target_duty: u16,
    /// What duty we're actually setting
    pub actual_duty: u16,
    /// Whether the duty changed this update
    pub duty_changed: bool,
    /// Why we made this decision
    pub reason: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn test_fan_curve_standard_interpolation() {
        let curve = FanCurve::standard();

        // Below first point - should return first duty
        assert_eq!(curve.get_duty(40_00), Some(0));

        // At exact points
        assert_eq!(curve.get_duty(45_00), Some(30_00));
        assert_eq!(curve.get_duty(88_00), Some(100_00));

        // Above last point
        assert_eq!(curve.get_duty(95_00), Some(100_00));

        // Interpolated value between 45C (30%) and 55C (35%)
        let duty_at_50 = curve.get_duty(50_00).unwrap();
        assert!(duty_at_50 > 30_00 && duty_at_50 < 35_00);
    }

    #[test]
    fn test_fan_curve_quiet() {
        let curve = FanCurve::quiet();

        // Fans off below 55C
        assert_eq!(curve.get_duty(50_00), Some(0));
        assert_eq!(curve.get_duty(54_99), Some(0));

        // Starts at 55C
        assert_eq!(curve.get_duty(55_00), Some(25_00));
    }

    #[test]
    fn test_controller_temperature_smoothing() {
        let curve = FanCurve::standard();
        let mut controller = FanController::new(curve)
            .with_smoothing_window(3)
            .with_ramp_up_delay(0.0)  // No delay for this test
            .with_ramp_down_delay(0.0);

        // Feed varying temperatures
        controller.update(60_00);  // 60C
        controller.update(70_00);  // 70C
        let output = controller.update(80_00);  // 80C

        // Smoothed should be average: (60+70+80)/3 = 70C
        assert_eq!(output.smoothed_temp, 70_00);
    }

    #[test]
    fn test_controller_smoothing_window_size() {
        let curve = FanCurve::standard();
        let mut controller = FanController::new(curve)
            .with_smoothing_window(2);

        // Fill window
        controller.update(60_00);
        controller.update(80_00);

        // Window is now [60, 80], avg = 70
        let output = controller.update(90_00);

        // Window should now be [80, 90], avg = 85 (60 was dropped)
        assert_eq!(output.smoothed_temp, 85_00);
    }

    #[test]
    fn test_controller_ramp_up_delay() {
        let curve = FanCurve::standard();
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_ramp_up_delay(0.1)  // 100ms delay
            .with_ramp_down_delay(10.0)
            .with_min_duty_change(0);  // React to any change

        // Start at moderate temp (50C = 32.5% duty)
        controller.update(50_00);
        sleep(Duration::from_millis(10));
        controller.update(50_00);

        // Small increase (55C = 35% duty) - only 2.5% change, should delay
        let output = controller.update(55_00);
        assert_eq!(output.reason, "ramp-up delayed");
        assert!(!output.duty_changed);

        // Wait for delay
        sleep(Duration::from_millis(150));

        // Now it should ramp up
        let output = controller.update(55_00);
        assert!(output.duty_changed);
        assert_eq!(output.reason, "ramp-up");
    }

    #[test]
    fn test_controller_ramp_down_delay() {
        let curve = FanCurve::standard();
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_ramp_up_delay(0.0)
            .with_ramp_down_delay(0.1)  // 100ms delay
            .with_min_duty_change(0);

        // Start hot, let it ramp up
        controller.update(88_00);  // 100% duty
        sleep(Duration::from_millis(10));
        let output = controller.update(88_00);
        assert_eq!(output.actual_duty, 100_00);

        // Go cold - should delay ramp down
        let output = controller.update(40_00);
        assert!(!output.duty_changed);
        assert_eq!(output.reason, "ramp-down delayed");
        assert_eq!(output.actual_duty, 100_00);  // Still at 100%

        // Wait for delay
        sleep(Duration::from_millis(150));

        // Now it should ramp down
        let output = controller.update(40_00);
        assert!(output.duty_changed);
        assert_eq!(output.reason, "ramp-down");
    }

    #[test]
    fn test_controller_immediate_large_jump() {
        let curve = FanCurve::standard();
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_ramp_up_delay(10.0)  // Long delay
            .with_min_duty_change(0);

        // Start at 0% duty
        controller.update(40_00);

        // Large jump (>10% duty change) should be immediate for safety
        let output = controller.update(88_00);  // Would be 100% duty

        // Should change immediately despite delay
        assert!(output.duty_changed);
    }

    #[test]
    fn test_controller_min_duty_change_threshold() {
        let curve = FanCurve::standard();
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_ramp_up_delay(0.0)
            .with_ramp_down_delay(0.0)
            .with_min_duty_change(5_00);  // 5% threshold

        // Get to a baseline
        controller.update(70_00);
        sleep(Duration::from_millis(10));
        let output = controller.update(70_00);
        let baseline_duty = output.actual_duty;

        // Small temp change that would cause <5% duty change
        let output = controller.update(71_00);

        // Should not change due to threshold
        assert!(!output.duty_changed);
        assert_eq!(output.actual_duty, baseline_duty);
    }

    #[test]
    fn test_controller_steady_state() {
        let curve = FanCurve::standard();
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_ramp_up_delay(0.0)
            .with_ramp_down_delay(0.0)
            .with_min_duty_change(0);

        // Reach steady state
        controller.update(70_00);
        sleep(Duration::from_millis(10));
        controller.update(70_00);

        // Same temp again
        let output = controller.update(70_00);

        assert_eq!(output.reason, "steady");
    }
}
