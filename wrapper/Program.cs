using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using OpenHardwareMonitor.Hardware;

namespace wrapper
{
    class Program
    {
        static void Main(string[] args)
        {
            bool debugMode = args.Length > 0 && args[0] == "--debug";

            var c = new Computer();
            c.CPUEnabled = true;
            c.GPUEnabled = true;
            c.Open();

            // Track max observed clock for throttle detection
            float maxObservedClock = 0;

            while (true) {
                // Wait for input - exits if stdin closes
                var line = Console.ReadLine();
                if (line == null) break;

                float? cpuCcdAvg = null;      // Preferred: pre-calculated CCD average
                var cpuCcdTemps = new List<float>();  // Fallback: individual CCD temps
                var gpuMax = 0.0F;
                string gpuMaxSource = "";

                // Clock and load tracking for throttle detection
                var cpuClocks = new List<float>();
                float cpuTotalLoad = 0;

                foreach (var h in c.Hardware) {
                    h.Update();
                    foreach (var s in h.Sensors) {
                        if (debugMode) {
                            var v = s.Value ?? 0;
                            Console.WriteLine($"  {h.HardwareType} | {s.SensorType} | {s.Name}: {v:F1}");
                        }

                        if (h.HardwareType == HardwareType.CPU) {
                            // Temperature sensors
                            if (s.SensorType == SensorType.Temperature) {
                                var v = s.Value ?? 0;
                                // Best: use pre-calculated CCD average (AMD Zen 2/3)
                                if (s.Name == "CPU CCD Average") {
                                    cpuCcdAvg = v;
                                }
                                // Fallback: collect individual CCD temps
                                else if (s.Name.StartsWith("CPU CCD #")) {
                                    cpuCcdTemps.Add(v);
                                }
                            }
                            // Clock sensors - collect per-core clocks
                            else if (s.SensorType == SensorType.Clock && s.Name.StartsWith("CPU Core #")) {
                                var v = s.Value ?? 0;
                                if (v > 0) cpuClocks.Add(v);
                            }
                            // Total CPU load
                            else if (s.SensorType == SensorType.Load && s.Name == "CPU Total") {
                                cpuTotalLoad = s.Value ?? 0;
                            }
                        }
                        // Track GPU max separately
                        else if (h.HardwareType == HardwareType.GpuNvidia || h.HardwareType == HardwareType.GpuAti) {
                            if (s.SensorType == SensorType.Temperature) {
                                var v = s.Value ?? 0;
                                if (v > gpuMax) {
                                    gpuMax = v;
                                    gpuMaxSource = $"{h.HardwareType}/{s.Name}";
                                }
                            }
                        }
                    }
                }

                // Use CCD average if available, otherwise calculate from individual CCDs
                float cpuTemp = cpuCcdAvg ?? (cpuCcdTemps.Count > 0 ? cpuCcdTemps.Average() : 0);

                // Use the higher of CPU average or GPU max
                float reportedTemp = Math.Max(cpuTemp, gpuMax);

                // Calculate average CPU clock and track max observed
                float avgClock = cpuClocks.Count > 0 ? cpuClocks.Average() : 0;
                if (avgClock > maxObservedClock) {
                    maxObservedClock = avgClock;
                }

                if (debugMode) {
                    Console.WriteLine($"CPU avg: {cpuTemp:F1}C (from {(cpuCcdAvg.HasValue ? "CCD Average sensor" : $"{cpuCcdTemps.Count} CCDs")})");
                    Console.WriteLine($"GPU max: {gpuMax:F1}C from {gpuMaxSource}");
                    Console.WriteLine($"CPU clock: {avgClock:F0} MHz (max seen: {maxObservedClock:F0} MHz)");
                    Console.WriteLine($"CPU load: {cpuTotalLoad:F1}%");
                    Console.WriteLine($"REPORTED: {reportedTemp:F1}C");
                } else {
                    // Output JSON for extended parsing (use invariant culture for consistent number formatting)
                    Console.WriteLine(string.Format(CultureInfo.InvariantCulture,
                        "{{\"temp\":{0:F2},\"cpu_clock\":{1},\"max_clock\":{2},\"cpu_load\":{3:F2}}}",
                        reportedTemp, (int)avgClock, (int)maxObservedClock, cpuTotalLoad));
                }
            }
        }
    }
}
