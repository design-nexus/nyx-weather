# Weather

Forecasts, hourly conditions, radar, air quality and severe-weather alerts for the places
you save, in a GTK 4 window.

## Features

- **Overview:** current conditions with feels-like and today's high and low, rain in the
  next two hours ("Rain in 15 min"), severe-weather alerts (US, from NOAA/NWS) with their
  area, times and instructions, a 24-hour strip, a 7-day forecast with range bars, and
  expandable wind, humidity and pressure, UV, air quality and sun details.
- **Hourly & Graphs:** 48-hour charts of temperature and feels-like with the chance of
  rain, wind and gusts, humidity and cloud cover, and surface pressure; hover a chart for
  its values. An hour-by-hour table follows.
- **10-Day Outlook:** a high, low and rain chart and a card for each day.
- **Radar & Overlays:** a map with four layers: radar on the classic NWS scale (in the US
  the NOAA NEXRAD mosaic, a frame every 5 minutes for the past hour; elsewhere RainViewer,
  every 10 minutes for the past two hours), infrared satellite, and the current wind and the
  temperature with sea-level isobars, both drawn from a 7×7 grid of Open-Meteo points over
  the map. Zoom with the buttons, Ctrl+scroll, a double-click or a pinch; play the radar
  loop, which fades smoothly from frame to frame; set the overlay's opacity.
- **Air Quality & Pollen:** US AQI and European EAQI with guidance, a 24-hour AQI and PM2.5
  chart, particulates and trace gases, pollen (forecast for Europe only), and UV.
- **Sun, Moon & Models:** the sun's path and daylight, the moon's phase, and each day's
  high from the ECMWF, GFS and ICON models beside the blended forecast.
- **Saved locations** in the sidebar with their current temperature; right-click one to
  rename, move or remove it.
- **Notifications** for new Severe and Extreme alerts.
- **Units:** temperature, wind, precipitation, pressure, distance and the clock, defaulting
  to the system locale's.
- **Offline:** the last forecast for each place is kept, and shown (moved on to the
  current hour) when the service can't be reached.
- **Floating window:** Ctrl+M or Settings → Weather window floats it centered at 460×720.
- **Waybar and the Omarchy bar:** `--waybar` prints a Waybar module, and the installer adds a
  bar widget that shows the conditions and temperature (turn it on in Settings → Plugins).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/design-nexus/nyx-weather/main/install.sh | bash -s -- weather
```

This builds with Cargo and installs `nyx-weather` to `~/.local/bin` with a launcher
entry. To remove it, run the same line with `uninstall.sh` in place of `install.sh`.

## Usage

```
nyx-weather                      Open or focus Weather
nyx-weather --toggle             Open, or close it if it's open
nyx-weather --section NAME       overview, hourly, daily, radar, air-quality, astronomy, settings
nyx-weather --location "Tokyo"   Switch to a saved place, or find and save it
nyx-weather --waybar             Waybar JSON, fetching first if the forecast is stale
nyx-weather --waybar-cached      Waybar JSON from the cache only
nyx-weather --refresh            Refresh every saved place and exit
```

## Keys

| Key | Action |
| --- | --- |
| Ctrl+F | Search for a place (Enter saves and opens the first result) |
| Ctrl+R, F5 | Refresh |
| Ctrl+1 … Ctrl+6 | Overview, Hourly, 10-Day, Radar, Air Quality, Sun & Moon |
| Alt+1 … Alt+9 | Switch saved place |
| Alt+↑, Alt+↓ | Move the active place up or down |
| Ctrl+M | Floating or tiled window |
| Ctrl+B | Collapse the sidebar |
| Ctrl+, | Settings |
| F1, ? | Keyboard shortcuts |
| Ctrl+Q, Ctrl+W | Close |

## Files

- Settings: `~/.config/nyx-weather/settings.toml`
- Forecast, search, map tile and grid cache: `~/.cache/nyx-weather/weather.db`
- `nyx weather …`, Glimpse (its Weather panel) and Nyx Assistant work Weather over the session bus: `weather.current`, `weather.forecast`, `weather.query`, `weather.locations`, `weather.select_location`, `weather.refresh` and `weather.open` (the names without `weather.` still work). D-Bus starts Weather without a window for them, and it quits once idle.

## Data

- Forecasts, air quality, geocoding and the map grids: [Open-Meteo](https://open-meteo.com/),
  CC BY 4.0. The free service is for non-commercial use; with an API key (Settings →
  Units & forecast) requests go to Open-Meteo's customer servers, or point it at your own.
- Radar and satellite: NOAA NEXRAD and GOES via the
  [Iowa Environmental Mesonet](https://mesonet.agron.iastate.edu/), and
  [RainViewer](https://www.rainviewer.com/api.html) outside the US.
- Map: © [OpenStreetMap](https://www.openstreetmap.org/copyright) contributors.
- US alerts: [NOAA / National Weather Service](https://www.weather.gov/documentation/services-web-api).
