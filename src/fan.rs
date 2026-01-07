// From https://github.com/pop-os/system76-power/blob/master/src/fan.rs
//TODO: use a shared crate

use std::collections::VecDeque;
use std::time::Instant;

/// Thermal throttling status
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ThrottleStatus {
    /// No throttling detected
    None,
    /// Throttling likely - high load but clock significantly below max
    Likely,
    /// Throttling confirmed - sustained clock reduction under load
    Confirmed,
}

impl ThrottleStatus {
    /// Returns true if any throttling is detected
    pub fn is_throttling(&self) -> bool {
        !matches!(self, ThrottleStatus::None)
    }
}

/// Sensor data from the hardware monitor wrapper
#[derive(Clone, Debug, Default)]
pub struct SensorData {
    /// Combined temperature - max of CPU and GPU (hundredths of a degree)
    pub temp: i16,
    /// CPU temperature (hundredths of a degree)
    pub cpu_temp: i16,
    /// Current average CPU clock in MHz
    pub cpu_clock: u32,
    /// Maximum observed CPU clock in MHz (approximates max boost)
    pub cpu_max_clock: u32,
    /// CPU load percentage (0-100)
    pub cpu_load: f32,
    /// GPU temperature (hundredths of a degree)
    pub gpu_temp: i16,
    /// Current GPU core clock in MHz
    pub gpu_clock: u32,
    /// Maximum observed GPU clock in MHz
    pub gpu_max_clock: u32,
    /// GPU load percentage (0-100)
    pub gpu_load: f32,
}

impl SensorData {
    /// Parse sensor data from JSON wrapper output
    pub fn from_json(json: &str) -> Result<Self, String> {
        // Simple JSON parsing without serde dependency
        let json = json.trim();

        let temp = Self::extract_float(json, "temp")?;
        let cpu_temp = Self::extract_float(json, "cpu_temp")?;
        let cpu_clock = Self::extract_int(json, "cpu_clock")?;
        let cpu_max_clock = Self::extract_int(json, "cpu_max_clock")?;
        let cpu_load = Self::extract_float(json, "cpu_load")?;
        let gpu_temp = Self::extract_float(json, "gpu_temp")?;
        let gpu_clock = Self::extract_int(json, "gpu_clock")?;
        let gpu_max_clock = Self::extract_int(json, "gpu_max_clock")?;
        let gpu_load = Self::extract_float(json, "gpu_load")?;

        Ok(Self {
            temp: (temp * 100.0) as i16,
            cpu_temp: (cpu_temp * 100.0) as i16,
            cpu_clock: cpu_clock as u32,
            cpu_max_clock: cpu_max_clock as u32,
            cpu_load: cpu_load as f32,
            gpu_temp: (gpu_temp * 100.0) as i16,
            gpu_clock: gpu_clock as u32,
            gpu_max_clock: gpu_max_clock as u32,
            gpu_load: gpu_load as f32,
        })
    }

    fn extract_float(json: &str, key: &str) -> Result<f64, String> {
        let pattern = format!("\"{}\":", key);
        let start = json.find(&pattern)
            .ok_or_else(|| format!("Key '{}' not found", key))?
            + pattern.len();

        let rest = &json[start..];
        let end = rest.find(|c: char| c == ',' || c == '}')
            .unwrap_or(rest.len());

        rest[..end].trim().parse::<f64>()
            .map_err(|e| format!("Failed to parse '{}': {}", key, e))
    }

    fn extract_int(json: &str, key: &str) -> Result<i64, String> {
        let pattern = format!("\"{}\":", key);
        let start = json.find(&pattern)
            .ok_or_else(|| format!("Key '{}' not found", key))?
            + pattern.len();

        let rest = &json[start..];
        let end = rest.find(|c: char| c == ',' || c == '}')
            .unwrap_or(rest.len());

        rest[..end].trim().parse::<i64>()
            .map_err(|e| format!("Failed to parse '{}': {}", key, e))
    }
}

/// Throttle detector with history tracking for CPU and GPU
pub struct ThrottleDetector {
    /// Number of consecutive samples showing CPU throttle-like behavior
    cpu_throttle_count: u32,
    /// Number of consecutive samples showing GPU throttle-like behavior
    gpu_throttle_count: u32,
    /// Threshold: clock must be this % below max to indicate throttling
    clock_threshold_pct: f32,
    /// Threshold: load must be above this % to consider throttling
    load_threshold_pct: f32,
    /// Threshold: temp must be above this to consider THERMAL throttling (hundredths of a degree)
    /// Below this, clock reduction is likely PBO power limits, not thermal
    temp_threshold: i16,
    /// Samples needed to confirm throttling
    confirm_samples: u32,
}

impl ThrottleDetector {
    pub fn new() -> Self {
        Self {
            cpu_throttle_count: 0,
            gpu_throttle_count: 0,
            clock_threshold_pct: 92.0, // Clock below 92% of max = suspicious
            load_threshold_pct: 70.0,  // Only check when load > 70%
            temp_threshold: 75_00,     // Only flag thermal throttling if temp > 75°C
            confirm_samples: 1,        // React immediately
        }
    }

    /// Check if a component is throttling based on clock, load, and temperature
    fn check_throttle(&self, clock: u32, max_clock: u32, load: f32, temp: i16) -> bool {
        if max_clock == 0 {
            return false;
        }
        let clock_pct = (clock as f32 / max_clock as f32) * 100.0;
        let is_under_load = load >= self.load_threshold_pct;
        let clock_reduced = clock_pct < self.clock_threshold_pct;
        let is_hot = temp >= self.temp_threshold;

        // Only flag as thermal throttling if:
        // 1. Under load (>70%)
        // 2. Clock is reduced (<92% of sustained max)
        // 3. Temperature is high (>75°C) - otherwise it's just PBO power limits
        is_under_load && clock_reduced && is_hot
    }

    /// Analyze sensor data and return throttle status (checks both CPU and GPU)
    pub fn analyze(&mut self, data: &SensorData) -> ThrottleStatus {
        // Check CPU throttling (use CPU temp)
        let cpu_throttling = self.check_throttle(
            data.cpu_clock, data.cpu_max_clock, data.cpu_load, data.cpu_temp
        );

        // Check GPU throttling (use GPU temp)
        let gpu_throttling = self.check_throttle(
            data.gpu_clock, data.gpu_max_clock, data.gpu_load, data.gpu_temp
        );

        // Update counters
        if cpu_throttling {
            self.cpu_throttle_count += 1;
        } else {
            self.cpu_throttle_count = 0;
        }

        if gpu_throttling {
            self.gpu_throttle_count += 1;
        } else {
            self.gpu_throttle_count = 0;
        }

        // Return worst status between CPU and GPU
        let cpu_status = self.count_to_status(self.cpu_throttle_count);
        let gpu_status = self.count_to_status(self.gpu_throttle_count);

        // Return the more severe status
        match (cpu_status, gpu_status) {
            (ThrottleStatus::Confirmed, _) | (_, ThrottleStatus::Confirmed) => ThrottleStatus::Confirmed,
            (ThrottleStatus::Likely, _) | (_, ThrottleStatus::Likely) => ThrottleStatus::Likely,
            _ => ThrottleStatus::None,
        }
    }

    fn count_to_status(&self, count: u32) -> ThrottleStatus {
        if count >= self.confirm_samples {
            ThrottleStatus::Confirmed
        } else if count > 0 {
            ThrottleStatus::Likely
        } else {
            ThrottleStatus::None
        }
    }

    /// Reset the detector state
    pub fn reset(&mut self) {
        self.cpu_throttle_count = 0;
        self.gpu_throttle_count = 0;
    }

    /// Check if CPU is currently throttling
    pub fn is_cpu_throttling(&self) -> bool {
        self.cpu_throttle_count > 0
    }

    /// Check if GPU is currently throttling
    pub fn is_gpu_throttling(&self) -> bool {
        self.gpu_throttle_count > 0
    }
}

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

    /// The standard fan curve optimized for PBO @ 85°C
    /// - 40-60°C: 0% → 40% (ramp to near-silent, 40% is almost silent)
    /// - 60-75°C: 40% → 60% (slow ramp, still quiet)
    /// - 75-85°C: 60% → 100% (smooth ramp to aggressive)
    pub fn standard() -> Self {
        Self::default()
            .append(40_00, 0_00)    // 0% at 40°C
            .append(60_00, 40_00)   // 40% at 60°C (near-silent)
            .append(65_00, 47_00)   // 47% at 65°C
            .append(70_00, 53_00)   // 53% at 70°C
            .append(75_00, 60_00)   // 60% at 75°C (start aggressive ramp)
            .append(78_00, 72_00)   // 72% at 78°C
            .append(80_00, 80_00)   // 80% at 80°C
            .append(82_00, 88_00)   // 88% at 82°C
            .append(84_00, 95_00)   // 95% at 84°C
            .append(85_00, 100_00)  // 100% at 85°C (matches PBO limit)
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

    /// PBO-based fan curve with configurable parameters.
    ///
    /// Creates a simple curve:
    /// - 40°C = 0%
    /// - 60°C = silence% (linear interpolation from 40-60°C)
    /// - PBO temp = max_fan% (linear interpolation from 60°C to PBO)
    ///
    /// Arguments are in hundredths (e.g., 85_00 = 85°C, 40_00 = 40%)
    pub fn pbo_curve(pbo_temp: i16, max_fan_duty: u16, silence_threshold: u16) -> Self {
        Self::default()
            .append(40_00, 0_00)           // 0% at 40°C
            .append(60_00, silence_threshold) // silence% at 60°C
            .append(pbo_temp, max_fan_duty)   // max_fan% at PBO temp
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

/// Fan controller with PBO-based curve and gradual ramp.
///
/// Designed around PBO temperature limits to avoid jet-engine behavior:
/// - Fast ramp to silence threshold (inaudible range)
/// - Hold at silence% for transient spikes
/// - Gradual +1%/sec ramp when sustained at PBO temp with 100% CPU load
/// - Critical override at PBO+2°C for thermal emergencies
pub struct FanController {
    curve: FanCurve,
    /// Rolling window of temperature readings (in hundredths of a degree)
    temp_history: VecDeque<i16>,
    /// How many samples to average (e.g., 5 = 5 seconds at 1 sample/sec)
    smoothing_window: usize,
    /// Current fan duty (hundredths of a percent)
    current_duty: u16,
    /// When we last decreased fan speed
    last_decrease: Option<Instant>,
    /// Seconds to wait before ramping down
    ramp_down_delay_secs: f32,
    /// Minimum duty change to act on (prevents micro-adjustments)
    min_duty_change: u16,
    /// PBO temperature limit (hundredths of a degree, e.g., 85_00 = 85°C)
    pbo_temp: i16,
    /// Maximum fan duty (hundredths of a percent, e.g., 100_00 = 100%)
    max_fan_duty: u16,
    /// Silence threshold - fans inaudible below this (hundredths of a percent)
    silence_threshold: u16,
    /// When sustained load (high CPU at PBO temp) started
    sustained_load_start: Option<Instant>,
    /// CPU load threshold for sustained load detection (0-100)
    sustained_load_threshold: f32,
}

impl FanController {
    /// Create a new fan controller with default PBO-based settings.
    ///
    /// Defaults:
    /// - 5 second temperature smoothing window
    /// - 10 second ramp-down delay
    /// - 200 (2%) minimum duty change
    /// - PBO temp: 85°C
    /// - Max fan duty: 100%
    /// - Silence threshold: 40%
    /// - Sustained load threshold: 50% CPU
    pub fn new(curve: FanCurve) -> Self {
        Self {
            curve,
            temp_history: VecDeque::with_capacity(10),
            smoothing_window: 5,
            current_duty: 0,
            last_decrease: None,
            ramp_down_delay_secs: 10.0,
            min_duty_change: 2_00, // 2%
            pbo_temp: 85_00,       // 85°C
            max_fan_duty: 100_00,  // 100%
            silence_threshold: 40_00, // 40%
            sustained_load_start: None,
            sustained_load_threshold: 50.0, // 50% CPU
        }
    }

    /// Configure the smoothing window size (in samples, typically seconds)
    pub fn with_smoothing_window(mut self, samples: usize) -> Self {
        self.smoothing_window = samples.max(1);
        self.temp_history = VecDeque::with_capacity(samples + 5);
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

    /// Configure PBO temperature limit (in hundredths of a degree)
    pub fn with_pbo_temp(mut self, temp: i16) -> Self {
        self.pbo_temp = temp;
        self
    }

    /// Configure maximum fan duty (in hundredths of a percent)
    pub fn with_max_fan_duty(mut self, duty: u16) -> Self {
        self.max_fan_duty = duty;
        self
    }

    /// Configure silence threshold (in hundredths of a percent)
    pub fn with_silence_threshold(mut self, duty: u16) -> Self {
        self.silence_threshold = duty;
        self
    }

    /// Configure sustained load CPU threshold (0-100%)
    pub fn with_sustained_load_threshold(mut self, load: f32) -> Self {
        self.sustained_load_threshold = load;
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

    /// Update the controller with a new temperature reading (simple interface).
    /// For full sensor data including throttle detection, use update_with_sensors().
    pub fn update(&mut self, temp: i16) -> FanControllerOutput {
        let data = SensorData {
            temp,
            cpu_temp: temp,
            cpu_clock: 0,
            cpu_max_clock: 0,
            cpu_load: 0.0,
            gpu_temp: 0,
            gpu_clock: 0,
            gpu_max_clock: 0,
            gpu_load: 0.0,
        };
        self.update_with_sensors(&data)
    }

    /// Seconds of sustained load required before allowing ramp above silence threshold
    const SUSTAINED_LOAD_REQUIRED_SECS: f32 = 10.0;

    /// Ramp rate when sustained load detected: duty increase per second (hundredths)
    /// 1% per second = 60 seconds from silence (40%) to max (100%)
    const RAMP_RATE_PER_SEC: u16 = 1_00;

    /// Update the controller with full sensor data.
    /// Returns the duty cycle to set (if it should change) and debug info.
    pub fn update_with_sensors(&mut self, data: &SensorData) -> FanControllerOutput {
        let now = Instant::now();
        let temp = data.temp;

        // Critical temperature = PBO + 2°C - bypass all delays for safety
        let critical_temp = self.pbo_temp + 2_00;
        if temp >= critical_temp {
            self.temp_history.push_back(temp);
            while self.temp_history.len() > self.smoothing_window {
                self.temp_history.pop_front();
            }

            // Force 100% immediately (always 100%, not max_fan_duty - this is emergency)
            let was_duty = self.current_duty;
            self.current_duty = 100_00;
            self.last_decrease = None;
            self.sustained_load_start = None;

            return FanControllerOutput {
                instant_temp: temp,
                smoothed_temp: self.smoothed_temp(),
                target_duty: 100_00,
                actual_duty: 100_00,
                duty_changed: was_duty != 100_00,
                reason: "CRITICAL TEMP",
                sustained_load: false,
                sustained_secs: 0.0,
                cpu_temp: data.cpu_temp,
                cpu_load: data.cpu_load,
                gpu_temp: data.gpu_temp,
                gpu_load: data.gpu_load,
            };
        }

        // Add to history, maintain window size
        self.temp_history.push_back(temp);
        while self.temp_history.len() > self.smoothing_window {
            self.temp_history.pop_front();
        }

        let smoothed_temp = self.smoothed_temp();
        let target_duty = self.curve.get_duty(smoothed_temp).unwrap_or(0)
            .min(self.max_fan_duty); // Cap at max_fan_duty

        // Check for sustained load condition: high CPU AND temp >= PBO temp
        let is_at_pbo_load = data.cpu_load >= self.sustained_load_threshold && smoothed_temp >= self.pbo_temp;

        // Track sustained load duration
        let sustained_secs = if is_at_pbo_load {
            match self.sustained_load_start {
                Some(start) => now.duration_since(start).as_secs_f32(),
                None => {
                    self.sustained_load_start = Some(now);
                    0.0
                }
            }
        } else {
            self.sustained_load_start = None;
            0.0
        };

        let sustained_load = sustained_secs >= Self::SUSTAINED_LOAD_REQUIRED_SECS;

        let mut output = FanControllerOutput {
            instant_temp: temp,
            smoothed_temp,
            target_duty,
            actual_duty: self.current_duty,
            duty_changed: false,
            reason: "holding",
            sustained_load,
            sustained_secs,
            cpu_temp: data.cpu_temp,
            cpu_load: data.cpu_load,
            gpu_temp: data.gpu_temp,
            gpu_load: data.gpu_load,
        };

        if target_duty > self.current_duty {
            // Want to increase fan speed
            if self.current_duty < self.silence_threshold {
                // Below silence threshold - ramp quickly (it's inaudible)
                self.current_duty = target_duty.min(self.silence_threshold);
                self.last_decrease = None;
                output.actual_duty = self.current_duty;
                output.duty_changed = true;
                output.reason = "ramp-up (quiet)";
            } else if sustained_load {
                // Above silence threshold with sustained load - gradual +1%/sec
                let new_duty = (self.current_duty + Self::RAMP_RATE_PER_SEC).min(target_duty);
                if new_duty != self.current_duty {
                    self.current_duty = new_duty;
                    self.last_decrease = None;
                    output.actual_duty = self.current_duty;
                    output.duty_changed = true;
                    output.reason = "ramp-up (sustained)";
                }
            } else {
                // Above silence threshold without sustained load - hold at silence
                if self.current_duty > self.silence_threshold {
                    // Already above silence, just hold
                    output.reason = "holding (no sustained load)";
                } else {
                    output.reason = "holding at silence";
                }
            }
        } else if target_duty < self.current_duty {
            // Want to decrease fan speed - use existing ramp-down delay
            let should_decrease = match self.last_decrease {
                None => {
                    self.last_decrease = Some(now);
                    false
                }
                Some(last) => {
                    now.duration_since(last).as_secs_f32() >= self.ramp_down_delay_secs
                }
            };

            if should_decrease {
                self.current_duty = target_duty;
                self.last_decrease = Some(now);
                output.actual_duty = target_duty;
                output.duty_changed = true;
                output.reason = "ramp-down";
            } else {
                output.reason = "ramp-down delayed";
            }
        } else {
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
    /// Whether sustained load condition is active
    pub sustained_load: bool,
    /// Seconds of sustained load (0 if not sustained)
    pub sustained_secs: f32,
    /// CPU temperature (hundredths of a degree)
    pub cpu_temp: i16,
    /// CPU load percentage
    pub cpu_load: f32,
    /// GPU temperature (hundredths of a degree)
    pub gpu_temp: i16,
    /// GPU load percentage
    pub gpu_load: f32,
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
        assert_eq!(curve.get_duty(30_00), Some(0));

        // At exact points
        assert_eq!(curve.get_duty(40_00), Some(0));       // 0% at 40°C
        assert_eq!(curve.get_duty(60_00), Some(40_00));   // 40% at 60°C
        assert_eq!(curve.get_duty(75_00), Some(60_00));   // 60% at 75°C
        assert_eq!(curve.get_duty(85_00), Some(100_00));  // 100% at 85°C

        // Above last point
        assert_eq!(curve.get_duty(95_00), Some(100_00));

        // Interpolated value between 60C (40%) and 65C (47%)
        let duty_at_62 = curve.get_duty(62_00).unwrap();
        assert!(duty_at_62 > 40_00 && duty_at_62 < 47_00);
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
        let curve = FanCurve::pbo_curve(85_00, 100_00, 40_00);
        let mut controller = FanController::new(curve)
            .with_smoothing_window(3)
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
        let curve = FanCurve::pbo_curve(85_00, 100_00, 40_00);
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
    fn test_controller_ramp_to_silence() {
        let curve = FanCurve::pbo_curve(85_00, 100_00, 40_00);
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_pbo_temp(85_00)
            .with_silence_threshold(40_00);

        // Start cold
        controller.update(40_00);

        // Go to 60C - should ramp quickly to 40% (silence threshold)
        let output = controller.update(60_00);
        assert!(output.duty_changed);
        assert_eq!(output.actual_duty, 40_00);
        assert_eq!(output.reason, "ramp-up (quiet)");
    }

    #[test]
    fn test_controller_holds_at_silence_without_sustained_load() {
        let curve = FanCurve::pbo_curve(85_00, 100_00, 40_00);
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_pbo_temp(85_00)
            .with_silence_threshold(40_00);

        // Get to silence threshold
        controller.update(60_00);

        // Go higher (75C) but without sustained load - should hold at silence
        let data = SensorData {
            temp: 75_00,
            cpu_temp: 75_00,
            cpu_clock: 0,
            cpu_max_clock: 0,
            cpu_load: 50.0,  // Not 100% load
            gpu_temp: 60_00,
            gpu_clock: 0,
            gpu_max_clock: 0,
            gpu_load: 30.0,
        };
        let output = controller.update_with_sensors(&data);

        // Should hold at silence threshold
        assert_eq!(output.actual_duty, 40_00);
        assert!(output.reason.contains("holding"));
    }

    #[test]
    fn test_controller_ramp_down_delay() {
        let curve = FanCurve::pbo_curve(85_00, 100_00, 40_00);
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_ramp_down_delay(0.1)  // 100ms delay
            .with_pbo_temp(85_00);

        // Start at silence threshold
        controller.update(60_00);
        let output = controller.update(60_00);
        assert_eq!(output.actual_duty, 40_00);

        // Go cold - should delay ramp down
        let output = controller.update(40_00);
        assert!(!output.duty_changed);
        assert_eq!(output.reason, "ramp-down delayed");

        // Wait for delay
        sleep(Duration::from_millis(150));

        // Now it should ramp down
        let output = controller.update(40_00);
        assert!(output.duty_changed);
        assert_eq!(output.reason, "ramp-down");
    }

    #[test]
    fn test_controller_critical_temp_override() {
        // Critical temp = PBO + 2°C = 87°C
        let curve = FanCurve::pbo_curve(85_00, 100_00, 40_00);
        let mut controller = FanController::new(curve)
            .with_smoothing_window(5)  // Long smoothing
            .with_pbo_temp(85_00);

        // Start cold
        controller.update(40_00);

        // Critical temp (87°C = PBO+2) should immediately jump to 100%
        let output = controller.update(87_00);
        assert!(output.duty_changed);
        assert_eq!(output.actual_duty, 100_00);
        assert_eq!(output.reason, "CRITICAL TEMP");

        // Even higher
        let output = controller.update(95_00);
        assert_eq!(output.actual_duty, 100_00);
        assert_eq!(output.reason, "CRITICAL TEMP");
    }

    #[test]
    fn test_controller_steady_state() {
        let curve = FanCurve::pbo_curve(85_00, 100_00, 40_00);
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_ramp_down_delay(0.0);

        // Reach steady state at silence threshold
        controller.update(60_00);
        controller.update(60_00);

        // Same temp again
        let output = controller.update(60_00);
        assert_eq!(output.reason, "steady");
    }

    #[test]
    fn test_pbo_curve_interpolation() {
        // 85°C PBO, 100% max, 40% silence
        let curve = FanCurve::pbo_curve(85_00, 100_00, 40_00);

        // Below 40C - should return 0%
        assert_eq!(curve.get_duty(30_00), Some(0));

        // At exact points
        assert_eq!(curve.get_duty(40_00), Some(0));        // 0% at 40°C
        assert_eq!(curve.get_duty(60_00), Some(40_00));    // 40% at 60°C
        assert_eq!(curve.get_duty(85_00), Some(100_00));   // 100% at 85°C

        // Interpolated: 72.5°C should be 70% (midpoint between 60°C/40% and 85°C/100%)
        let duty_at_72 = curve.get_duty(72_50).unwrap();
        assert!(duty_at_72 > 60_00 && duty_at_72 < 80_00);
    }

    #[test]
    fn test_max_fan_duty_cap() {
        // 85°C PBO, 50% max (capped), 40% silence
        let curve = FanCurve::pbo_curve(85_00, 50_00, 40_00);
        let mut controller = FanController::new(curve)
            .with_smoothing_window(1)
            .with_pbo_temp(85_00)
            .with_max_fan_duty(50_00)
            .with_silence_threshold(40_00);

        // At 85°C, should cap at 50% not 100%
        let output = controller.update(85_00);
        assert!(output.actual_duty <= 50_00);
    }

    #[test]
    fn test_sensor_data_from_json() {
        let json = r#"{"temp":85.50,"cpu_temp":82.00,"cpu_clock":3200,"cpu_max_clock":4500,"cpu_load":95.25,"gpu_temp":75.00,"gpu_clock":1800,"gpu_max_clock":2000,"gpu_load":80.00}"#;
        let data = SensorData::from_json(json).unwrap();

        assert_eq!(data.temp, 85_50); // 85.50C in hundredths
        assert_eq!(data.cpu_temp, 82_00);
        assert_eq!(data.cpu_clock, 3200);
        assert_eq!(data.cpu_max_clock, 4500);
        assert!((data.cpu_load - 95.25).abs() < 0.01);
        assert_eq!(data.gpu_temp, 75_00);
        assert_eq!(data.gpu_clock, 1800);
        assert_eq!(data.gpu_max_clock, 2000);
        assert!((data.gpu_load - 80.0).abs() < 0.01);
    }

    #[test]
    fn test_sensor_data_from_json_integer_values() {
        // Test with integer values (no decimal points)
        let json = r#"{"temp":90,"cpu_temp":88,"cpu_clock":4000,"cpu_max_clock":4500,"cpu_load":50,"gpu_temp":70,"gpu_clock":1500,"gpu_max_clock":2000,"gpu_load":30}"#;
        let data = SensorData::from_json(json).unwrap();

        assert_eq!(data.temp, 90_00);
        assert_eq!(data.cpu_temp, 88_00);
        assert_eq!(data.cpu_clock, 4000);
        assert_eq!(data.cpu_max_clock, 4500);
        assert!((data.cpu_load - 50.0).abs() < 0.01);
        assert_eq!(data.gpu_temp, 70_00);
        assert_eq!(data.gpu_clock, 1500);
        assert_eq!(data.gpu_max_clock, 2000);
        assert!((data.gpu_load - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_throttle_detector_no_throttling() {
        let mut detector = ThrottleDetector::new();

        // Normal operation: high clock (>92%), any load
        let data = SensorData {
            temp: 70_00,
            cpu_temp: 70_00,
            cpu_clock: 4300,  // 95.5% of max - above 92% threshold
            cpu_max_clock: 4500,
            cpu_load: 50.0,
            gpu_temp: 60_00,
            gpu_clock: 1900,  // 95% of max
            gpu_max_clock: 2000,
            gpu_load: 30.0,
        };
        assert_eq!(detector.analyze(&data), ThrottleStatus::None);

        // High load but clock still high (>92%)
        let data = SensorData {
            temp: 85_00,
            cpu_temp: 85_00,
            cpu_clock: 4200,  // 93.3% of max - above 92% threshold
            cpu_max_clock: 4500,
            cpu_load: 95.0,
            gpu_temp: 75_00,
            gpu_clock: 1900,  // 95% of max
            gpu_max_clock: 2000,
            gpu_load: 85.0,
        };
        assert_eq!(detector.analyze(&data), ThrottleStatus::None);
    }

    #[test]
    fn test_throttle_detector_immediate_detection() {
        let mut detector = ThrottleDetector::new();

        // High load with reduced CPU clock (GPU normal)
        let data = SensorData {
            temp: 90_00,
            cpu_temp: 90_00,
            cpu_clock: 3000,  // 66% of max - well below 92% threshold
            cpu_max_clock: 4500,
            cpu_load: 95.0,
            gpu_temp: 70_00,
            gpu_clock: 1900,  // 95% - above threshold
            gpu_max_clock: 2000,
            gpu_load: 50.0,
        };

        // With confirm_samples=1, throttling is immediately confirmed
        assert_eq!(detector.analyze(&data), ThrottleStatus::Confirmed);
    }

    #[test]
    fn test_throttle_detector_confirmed_throttling() {
        let mut detector = ThrottleDetector::new();

        let data = SensorData {
            temp: 95_00,
            cpu_temp: 95_00,
            cpu_clock: 2800,  // ~62% of max - well below 92% threshold
            cpu_max_clock: 4500,
            cpu_load: 98.0,
            gpu_temp: 80_00,
            gpu_clock: 1800,  // 90% - also below 92% threshold
            gpu_max_clock: 2000,
            gpu_load: 75.0,   // Above 70% load threshold
        };

        // With confirm_samples=1, first sample immediately confirms throttling
        assert_eq!(detector.analyze(&data), ThrottleStatus::Confirmed);
    }

    #[test]
    fn test_throttle_detector_clears_on_recovery() {
        let mut detector = ThrottleDetector::new();

        // Build up throttle state
        let throttle_data = SensorData {
            temp: 95_00,
            cpu_temp: 95_00,
            cpu_clock: 2800,  // 62% of max - below 92% threshold
            cpu_max_clock: 4500,
            cpu_load: 98.0,
            gpu_temp: 80_00,
            gpu_clock: 1800,  // 90% - below 92% threshold
            gpu_max_clock: 2000,
            gpu_load: 75.0,   // Above 70% load threshold
        };
        assert_eq!(detector.analyze(&throttle_data), ThrottleStatus::Confirmed);

        // Now recover - clock returns to normal (>92%)
        let normal_data = SensorData {
            temp: 70_00,
            cpu_temp: 70_00,
            cpu_clock: 4400,  // 97.8% of max
            cpu_max_clock: 4500,
            cpu_load: 30.0,
            gpu_temp: 60_00,
            gpu_clock: 1900,  // 95% of max
            gpu_max_clock: 2000,
            gpu_load: 20.0,
        };
        assert_eq!(detector.analyze(&normal_data), ThrottleStatus::None);

        // Counter should be reset, so next throttle immediately confirms
        assert_eq!(detector.analyze(&throttle_data), ThrottleStatus::Confirmed);
    }

    #[test]
    fn test_throttle_detector_no_data() {
        let mut detector = ThrottleDetector::new();

        // No clock data available
        let data = SensorData {
            temp: 90_00,
            cpu_temp: 90_00,
            cpu_clock: 0,
            cpu_max_clock: 0,
            cpu_load: 95.0,
            gpu_temp: 80_00,
            gpu_clock: 0,
            gpu_max_clock: 0,
            gpu_load: 80.0,
        };
        assert_eq!(detector.analyze(&data), ThrottleStatus::None);
    }

    #[test]
    fn test_throttle_detector_gpu_throttling() {
        let mut detector = ThrottleDetector::new();

        // CPU fine but GPU throttling
        let data = SensorData {
            temp: 85_00,
            cpu_temp: 70_00,
            cpu_clock: 4400,  // 97.8% - above 92% threshold
            cpu_max_clock: 4500,
            cpu_load: 30.0,
            gpu_temp: 85_00,
            gpu_clock: 1400,  // 70% of max - well below 92% threshold
            gpu_max_clock: 2000,
            gpu_load: 95.0,   // Above 70% load threshold
        };

        // GPU throttling immediately confirmed
        assert_eq!(detector.analyze(&data), ThrottleStatus::Confirmed);
        assert!(detector.is_gpu_throttling());
        assert!(!detector.is_cpu_throttling());
    }
}
