using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using LibreHardwareMonitor.Hardware;

namespace wrapper
{
    class Program
    {
        static void Main(string[] args)
        {
            bool debugMode = args.Length > 0 && args[0] == "--debug";

            var c = new Computer
            {
                IsCpuEnabled = true,
                IsGpuEnabled = true
            };
            c.Open();

            // Track max observed clocks for throttle detection
            float maxObservedCpuClock = 0;
            float maxObservedGpuClock = 0;

            while (true) {
                // Wait for input - exits if stdin closes
                var line = Console.ReadLine();
                if (line == null) break;

                float? cpuPackageTemp = null;  // Intel: "CPU Package" or AMD: "Core (Tctl/Tdie)"
                float? cpuCoreMax = null;     // "Core Max" - hottest core
                float? cpuCcdAvg = null;      // AMD Zen 2/3: pre-calculated CCD average
                var cpuCoreTemps = new List<float>();  // Individual core temps
                var cpuCcdTemps = new List<float>();   // AMD: individual CCD temps
                float gpuTemp = 0;
                string gpuTempSource = "";
                string cpuTempSource = "";

                // CPU clock and load tracking for throttle detection
                var cpuClocks = new List<float>();
                float cpuTotalLoad = 0;

                // GPU clock and load tracking for throttle detection
                float gpuCoreClock = 0;
                float gpuCoreLoad = 0;

                foreach (var h in c.Hardware) {
                    h.Update();
                    foreach (var s in h.Sensors) {
                        if (debugMode) {
                            var v = s.Value ?? 0;
                            Console.WriteLine($"  {h.HardwareType} | {s.SensorType} | {s.Name}: {v:F1}");
                        }

                        if (h.HardwareType == HardwareType.Cpu) {
                            // Temperature sensors - capture all relevant ones
                            if (s.SensorType == SensorType.Temperature) {
                                var v = s.Value ?? 0;

                                // Intel: "CPU Package" is the main package temp
                                // AMD: "Core (Tctl/Tdie)" is the main temp
                                if (s.Name == "CPU Package" || s.Name == "Core (Tctl/Tdie)") {
                                    cpuPackageTemp = v;
                                }
                                // "Core Max" - hottest individual core
                                else if (s.Name == "Core Max") {
                                    cpuCoreMax = v;
                                }
                                // AMD Zen 2/3: pre-calculated CCD average
                                else if (s.Name == "CPU CCD Average") {
                                    cpuCcdAvg = v;
                                }
                                // Individual core temps (Intel: "Core #0", "Core #1", etc.)
                                else if (s.Name.StartsWith("Core #")) {
                                    cpuCoreTemps.Add(v);
                                }
                                // AMD: individual CCD temps
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
                        // Track GPU metrics
                        else if (h.HardwareType == HardwareType.GpuNvidia || h.HardwareType == HardwareType.GpuAmd || h.HardwareType == HardwareType.GpuIntel) {
                            var v = s.Value ?? 0;
                            if (s.SensorType == SensorType.Temperature) {
                                // Track hottest GPU temp
                                if (v > gpuTemp) {
                                    gpuTemp = v;
                                    gpuTempSource = $"{h.HardwareType}/{s.Name}";
                                }
                            }
                            else if (s.SensorType == SensorType.Clock && s.Name == "GPU Core") {
                                gpuCoreClock = v;
                            }
                            else if (s.SensorType == SensorType.Load && s.Name == "GPU Core") {
                                gpuCoreLoad = v;
                            }
                        }
                    }
                }

                // Select CPU temp with priority - Package temp is most stable for fan control
                // Priority: Package/Tctl > CCD Average > average of cores/CCDs
                // (Core Max spikes too much and causes fan oscillation)
                float cpuTemp = 0;
                if (cpuPackageTemp.HasValue) {
                    cpuTemp = cpuPackageTemp.Value;
                    cpuTempSource = "CPU Package";
                } else if (cpuCcdAvg.HasValue) {
                    cpuTemp = cpuCcdAvg.Value;
                    cpuTempSource = "CCD Average";
                } else if (cpuCoreTemps.Count > 0) {
                    cpuTemp = cpuCoreTemps.Average();
                    cpuTempSource = $"Avg of {cpuCoreTemps.Count} cores";
                } else if (cpuCcdTemps.Count > 0) {
                    cpuTemp = cpuCcdTemps.Average();
                    cpuTempSource = $"Avg of {cpuCcdTemps.Count} CCDs";
                } else if (cpuCoreMax.HasValue) {
                    // Last resort fallback
                    cpuTemp = cpuCoreMax.Value;
                    cpuTempSource = "Core Max (fallback)";
                }

                // Use the higher of CPU or GPU temp for fan control
                float reportedTemp = Math.Max(cpuTemp, gpuTemp);

                // Calculate average CPU clock and track max observed
                float avgCpuClock = cpuClocks.Count > 0 ? cpuClocks.Average() : 0;
                if (avgCpuClock > maxObservedCpuClock) {
                    maxObservedCpuClock = avgCpuClock;
                }

                // Track max observed GPU clock
                if (gpuCoreClock > maxObservedGpuClock) {
                    maxObservedGpuClock = gpuCoreClock;
                }

                if (debugMode) {
                    Console.WriteLine($"CPU temp: {cpuTemp:F1}C (from {cpuTempSource})");
                    Console.WriteLine($"  Package: {cpuPackageTemp?.ToString("F1") ?? "N/A"}C, Core Max: {cpuCoreMax?.ToString("F1") ?? "N/A"}C, CCD Avg: {cpuCcdAvg?.ToString("F1") ?? "N/A"}C");
                    Console.WriteLine($"  Core temps: [{string.Join(", ", cpuCoreTemps.Select(t => $"{t:F1}"))}]");
                    Console.WriteLine($"CPU clock: {avgCpuClock:F0} MHz (max seen: {maxObservedCpuClock:F0} MHz)");
                    Console.WriteLine($"CPU load: {cpuTotalLoad:F1}%");
                    Console.WriteLine($"GPU temp: {gpuTemp:F1}C from {gpuTempSource}");
                    Console.WriteLine($"GPU clock: {gpuCoreClock:F0} MHz (max seen: {maxObservedGpuClock:F0} MHz)");
                    Console.WriteLine($"GPU load: {gpuCoreLoad:F1}%");
                    Console.WriteLine($"REPORTED: {reportedTemp:F1}C");
                } else {
                    // Output JSON for extended parsing (use invariant culture for consistent number formatting)
                    Console.WriteLine(string.Format(CultureInfo.InvariantCulture,
                        "{{\"temp\":{0:F2},\"cpu_temp\":{1:F2},\"cpu_clock\":{2},\"cpu_max_clock\":{3},\"cpu_load\":{4:F2},\"gpu_temp\":{5:F2},\"gpu_clock\":{6},\"gpu_max_clock\":{7},\"gpu_load\":{8:F2}}}",
                        reportedTemp, cpuTemp, (int)avgCpuClock, (int)maxObservedCpuClock, cpuTotalLoad,
                        gpuTemp, (int)gpuCoreClock, (int)maxObservedGpuClock, gpuCoreLoad));
                }
            }
        }
    }
}
