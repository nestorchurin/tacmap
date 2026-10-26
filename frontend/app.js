/* TACMAP — зведена карта: живі цілі з джерел + пуски + тривоги.
   Рендер ТІЛЬКИ шарами GeoJSON через setData (повна заміна).
   Єдине місце координат — бекенд geo.rs ([lon, lat]). */

const API = "";

try {
  document.querySelectorAll("#build-warn, .build-warn, #delay-badge, .delay-badge").forEach((el) => el.remove());
} catch (e) {}

// Обмеження розміру карти: театр дій (Україна + європейська частина РФ з базами пусків)
// Жорстко забороняє скрол та рендер за межі театру (немає Америки, Африки, Сибіру, Західної Європи)
const MAP_BOUNDS = [
  [16.0, 40.0], // Southwest: західний кордон України (Карпати / Чоп) та Чорне море / Кавказ
  [65.0, 72.0], // Northeast: Урал на сході та Оленья / Мурманськ на півночі
];

const map = new maplibregl.Map({
  container: "map",
  renderWorldCopies: false,
  minZoom: 3.2,
  maxZoom: 16.5,
  maxBounds: MAP_BOUNDS,
  style: {
    // Чистий AMOLED-вектор: чорний фон + вода + межі + міста, жодних растрів
    version: 8,
    glyphs: "https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf",
    sources: {
      tiles: { type: "vector", url: "https://tiles.openfreemap.org/planet" },
    },
    layers: [
      { id: "bg", type: "background", paint: { "background-color": "#000" } },
      { id: "water", type: "fill", source: "tiles", "source-layer": "water",
        paint: { "fill-color": "#050505" } },
      { id: "ctry", type: "line", source: "tiles", "source-layer": "boundary",
        filter: ["==", ["get", "admin_level"], 2],
        paint: { "line-color": "#e8e0c8", "line-opacity": 0.85,
          "line-width": ["interpolate", ["exponential", 1.5], ["zoom"], 0, 0.8, 9, 1.6, 18, 5] } },
      { id: "obl", type: "line", source: "tiles", "source-layer": "boundary",
        filter: ["all", ["==", ["get", "admin_level"], 4], ["!=", ["get", "disputed"], true]],
        paint: { "line-color": "#d6c48a", "line-opacity": 0.3,
          "line-width": ["interpolate", ["linear"], ["zoom"], 5, 0.5, 9, 1, 13, 1.6],
          "line-dasharray": [3, 2] } },
      { id: "city", type: "symbol", source: "tiles", "source-layer": "place",
        minzoom: 4, maxzoom: 16,
        filter: ["==", ["get", "class"], "city"],
        layout: {
          "text-field": ["coalesce", ["get", "name:uk"], ["get", "name:en"], ["get", "name"]],
          "text-font": ["Noto Sans Bold"],
          "text-size": ["interpolate", ["linear"], ["zoom"], 4, 10, 8, 14, 11, 18],
        },
        paint: { "text-color": "#cfc8b0", "text-halo-color": "#000", "text-halo-width": 1.5 } },
      { id: "town", type: "symbol", source: "tiles", "source-layer": "place",
        minzoom: 7, maxzoom: 16,
        filter: ["in", ["get", "class"], ["literal", ["town", "village", "hamlet", "suburb"]]],
        layout: {
          "text-field": ["coalesce", ["get", "name:uk"], ["get", "name:en"], ["get", "name"]],
          "text-font": ["Noto Sans Bold"],
          "text-size": 11,
        },
        paint: { "text-color": "#8f8a76", "text-halo-color": "#000", "text-halo-width": 1.5 } },
    ],
  },
  center: [31.5, 49.0],
  zoom: 5.4,
  attributionControl: { compact: true },
});
map.on("error", (e) => console.warn("map error", e?.error?.message || e));
map.addControl(new maplibregl.NavigationControl({ visualizePitch: true }), "bottom-right");
map.addControl(new maplibregl.ScaleControl({ unit: "metric" }), "bottom-right");

let launchesOn = true;
// Останні події пусків + зсув годинника (сервер-клієнт) для живого «хв тому»
let lastEvents = [];
let lastCorridors = [];
let lastLaunches = [];
const activePulseMarkers = new Map(); // id -> { marker, elTag, info }
let clockOffsetSec = 0;
let activeTargetPopup = null;
let activeTargetId = null;
let lastClosedTargetId = null;
let lastClosedTime = 0;

// Кольори стрілок прольоту = кольори видів пусків (бэкенд launches.rs)
const ARROW_COLORS = {
  uav: "#ff7a00", missile: "#ff3b30", mig31k: "#ff3b30",
  ballistic: "#e879f9", naval: "#38e1ff",
};

// Стрілка прольоту: трикутник носом догори, крутиться за курсом (icon-rotate).
function makeArrow(color) {
  const c = document.createElement("canvas");
  c.width = 64; c.height = 64;
  const ctx = c.getContext("2d", { willReadFrequently: true });
  ctx.fillStyle = color;
  ctx.strokeStyle = "#ffffff";
  ctx.lineWidth = 5;
  ctx.lineJoin = "round";
  ctx.beginPath();
  ctx.moveTo(32, 6); ctx.lineTo(54, 56); ctx.lineTo(32, 44); ctx.lineTo(10, 56);
  ctx.closePath(); ctx.fill(); ctx.stroke();
  return ctx.getImageData(0, 0, c.width, c.height);
}

function makeArrows() {
  if (!map.hasImage("arrow-mono")) {
    map.addImage("arrow-mono", makeArrow("#64748b"));
  }
  if (!map.hasImage("arrow-alert")) {
    map.addImage("arrow-alert", makeArrow("#ff1111"));
  }
  for (const [kind, color] of Object.entries(ARROW_COLORS)) {
    const id = `arrow-${kind}`;
    if (!map.hasImage(id)) map.addImage(id, makeArrow(color));
  }
}

map.on("mousemove", (e) => {
  document.getElementById("coords").textContent =
    `${e.lngLat.lat.toFixed(4)} N, ${e.lngLat.lng.toFixed(4)} E · Z${map.getZoom().toFixed(1)}`;
});
document.getElementById("recenter").addEventListener("click", () => {
  map.flyTo({ center: [31.5, 49.0], zoom: 5.4 });
});

// ==========================================================================
// Стратегічні міста, райцентри та вузли України для проекції курсу цілей
// ==========================================================================
const MAJOR_CITIES = [
  { name: "Київ", lat: 50.4501, lon: 30.5234, region: "Київська" },
  { name: "Харків", lat: 49.9935, lon: 36.2304, region: "Харківська" },
  { name: "Одеса", lat: 46.4825, lon: 30.7233, region: "Одеська" },
  { name: "Дніпро", lat: 48.4647, lon: 35.0462, region: "Дніпропетровська" },
  { name: "Запоріжжя", lat: 47.8388, lon: 35.1396, region: "Запорізька" },
  { name: "Львів", lat: 49.8397, lon: 24.0297, region: "Львівська" },
  { name: "Кривий Ріг", lat: 47.9105, lon: 33.3918, region: "Дніпропетровська" },
  { name: "Миколаїв", lat: 46.9750, lon: 31.9946, region: "Миколаївська" },
  { name: "Вінниця", lat: 49.2331, lon: 28.4682, region: "Вінницька" },
  { name: "Полтава", lat: 49.5883, lon: 34.5514, region: "Полтавська" },
  { name: "Чернігів", lat: 51.4982, lon: 31.2893, region: "Чернігівська" },
  { name: "Черкаси", lat: 49.4444, lon: 32.0598, region: "Черкаська" },
  { name: "Хмельницький", lat: 49.4230, lon: 26.9871, region: "Хмельницька" },
  { name: "Житомир", lat: 50.2547, lon: 28.6587, region: "Житомирська" },
  { name: "Чернівці", lat: 48.2917, lon: 25.9352, region: "Чернівецька" },
  { name: "Суми", lat: 50.9077, lon: 34.7981, region: "Сумська" },
  { name: "Рівне", lat: 50.6199, lon: 26.2516, region: "Рівненська" },
  { name: "Івано-Франківськ", lat: 48.9226, lon: 24.7111, region: "Івано-Франківська" },
  { name: "Тернопіль", lat: 49.5535, lon: 25.5948, region: "Тернопільська" },
  { name: "Кропивницький", lat: 48.5079, lon: 32.2623, region: "Кіровоградська" },
  { name: "Кременчук", lat: 49.0703, lon: 33.4224, region: "Полтавська" },
  { name: "Луцьк", lat: 50.7472, lon: 25.3254, region: "Волинська" },
  { name: "Біла Церква", lat: 49.7989, lon: 30.1153, region: "Київська" },
  { name: "Ужгород", lat: 48.6208, lon: 22.2879, region: "Закарпатська" },
  { name: "Нікополь", lat: 47.5760, lon: 34.3948, region: "Дніпропетровська" },
  { name: "Павлоград", lat: 48.5255, lon: 35.8742, region: "Дніпропетровська" },
  { name: "Кам'янське", lat: 48.5164, lon: 34.6139, region: "Дніпропетровська" },
  { name: "Конотоп", lat: 51.2407, lon: 33.2032, region: "Сумська" },
  { name: "Умань", lat: 48.7484, lon: 30.2218, region: "Черкаська" },
  { name: "Мукачево", lat: 48.4412, lon: 22.7176, region: "Закарпатська" },
  { name: "Олександрія", lat: 48.6693, lon: 33.1156, region: "Кіровоградська" },
  { name: "Шостка", lat: 51.8631, lon: 33.4698, region: "Сумська" },
  { name: "Бердичів", lat: 49.8967, lon: 28.5832, region: "Житомирська" },
  { name: "Дрогобич", lat: 49.3541, lon: 23.5055, region: "Львівська" },
  { name: "Ніжин", lat: 51.0470, lon: 31.8812, region: "Чернігівська" },
  { name: "Ізмаїл", lat: 45.3516, lon: 28.8386, region: "Одеська" },
  { name: "Самар", lat: 48.6339, lon: 35.2285, region: "Дніпропетровська" },
  { name: "Ковель", lat: 51.2185, lon: 24.7171, region: "Волинська" },
  { name: "Сміла", lat: 49.2241, lon: 31.8741, region: "Черкаська" },
  { name: "Коростень", lat: 50.9501, lon: 28.6436, region: "Житомирська" },
  { name: "Коломия", lat: 48.5303, lon: 25.0401, region: "Івано-Франківська" },
  { name: "Стрий", lat: 49.2559, lon: 23.8569, region: "Львівська" },
  { name: "Чортків", lat: 49.0153, lon: 25.7979, region: "Тернопільська" },
  { name: "Прилуки", lat: 50.5902, lon: 32.3872, region: "Чернігівська" },
  { name: "Лозова", lat: 48.8911, lon: 36.3175, region: "Харківська" },
  { name: "Звягель", lat: 50.5878, lon: 27.6253, region: "Житомирська" },
  { name: "Енергодар", lat: 47.4988, lon: 34.6570, region: "Запорізька" },
  { name: "Бориспіль", lat: 50.3528, lon: 30.9547, region: "Київська" },
  { name: "Бровари", lat: 50.5113, lon: 30.7901, region: "Київська" },
  { name: "Фастів", lat: 50.0784, lon: 29.9161, region: "Київська" },
  { name: "Васильків", lat: 50.1805, lon: 30.3168, region: "Київська" },
  { name: "Ірпінь / Буча", lat: 50.5218, lon: 30.2505, region: "Київська" },
  { name: "Старокостянтинів", lat: 49.7567, lon: 27.2211, region: "Хмельницька" },
  { name: "Шепетівка", lat: 50.1833, lon: 27.0667, region: "Хмельницька" },
  { name: "Дубно", lat: 50.4194, lon: 25.7347, region: "Рівненська" },
  { name: "Сарни", lat: 51.3383, lon: 26.6022, region: "Рівненська" },
  { name: "Чорноморськ", lat: 46.2994, lon: 30.6558, region: "Одеська" },
  { name: "Південноукраїнськ", lat: 47.8219, lon: 31.1764, region: "Миколаївська" },
  { name: "Вознесенськ", lat: 47.5639, lon: 31.3283, region: "Миколаївська" },
  { name: "Первомайськ", lat: 48.0444, lon: 30.8506, region: "Миколаївська" },
  { name: "Миргород", lat: 49.9658, lon: 33.6075, region: "Полтавська" },
  { name: "Лубни", lat: 50.0167, lon: 33.0000, region: "Полтавська" },
  { name: "Охтирка", lat: 50.3111, lon: 34.8989, region: "Сумська" },
  { name: "Ромни", lat: 50.7511, lon: 33.4756, region: "Сумська" },
  { name: "Глухів", lat: 51.6778, lon: 33.9167, region: "Сумська" },
  { name: "Куп'янськ", lat: 49.7144, lon: 37.6167, region: "Харківська" },
  { name: "Ізюм", lat: 49.2089, lon: 37.2792, region: "Харківська" },
  { name: "Чугуїв", lat: 49.8356, lon: 36.6864, region: "Харківська" },
  { name: "Краматорськ", lat: 48.7390, lon: 37.5838, region: "Донецька" },
  { name: "Слов'янськ", lat: 48.8535, lon: 37.6251, region: "Донецька" },
  { name: "Покровськ", lat: 48.2828, lon: 37.1828, region: "Донецька" },
  { name: "Херсон", lat: 46.6354, lon: 32.6169, region: "Херсонська" },
  { name: "Берислав", lat: 46.8394, lon: 33.4278, region: "Херсонська" },
];

function geoHaversineKm(lat1, lon1, lat2, lon2) {
  const r = 6371.0;
  const dlat = ((lat2 - lat1) * Math.PI) / 180;
  const dlon = ((lon2 - lon1) * Math.PI) / 180;
  const a =
    Math.sin(dlat / 2) ** 2 +
    Math.cos((lat1 * Math.PI) / 180) *
      Math.cos((lat2 * Math.PI) / 180) *
      Math.sin(dlon / 2) ** 2;
  return 2 * r * Math.asin(Math.sqrt(a));
}

function geoBearingDeg(lat1, lon1, lat2, lon2) {
  const la1 = (lat1 * Math.PI) / 180;
  const la2 = (lat2 * Math.PI) / 180;
  const dlo = ((lon2 - lon1) * Math.PI) / 180;
  const y = Math.sin(dlo) * Math.cos(la2);
  const x =
    Math.cos(la1) * Math.sin(la2) -
    Math.sin(la1) * Math.cos(la2) * Math.cos(dlo);
  return (((Math.atan2(y, x) * 180) / Math.PI) + 360) % 360;
}

function geoDestPoint(lat, lon, bearingDeg, distKm) {
  const r = 6371.0;
  const br = (bearingDeg * Math.PI) / 180;
  const la1 = (lat * Math.PI) / 180;
  const lo1 = (lon * Math.PI) / 180;
  const d = distKm / r;
  const la2 = Math.asin(
    Math.sin(la1) * Math.cos(d) + Math.cos(la1) * Math.sin(d) * Math.cos(br)
  );
  const lo2 =
    lo1 +
    Math.atan2(
      Math.sin(br) * Math.sin(d) * Math.cos(la1),
      Math.cos(d) - Math.sin(la1) * Math.sin(la2)
    );
  const outLon = (((lo2 * 180) / Math.PI + 540) % 360) - 180;
  const outLat = (la2 * 180) / Math.PI;
  return [outLon, outLat];
}

function geoAngleDiff(a, b) {
  const d = Math.abs(a - b) % 360;
  return d > 180 ? 360 - d : d;
}

const activeCategories = new Set(["uav", "tactical", "missile", "ballistic", "kab", "recon_fpv", "launches"]);

function initTargetsGrid() {
  const grid = document.getElementById("targets-grid");
  if (!grid) return;
  grid.querySelectorAll(".grid-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const type = btn.dataset.type;
      if (activeCategories.has(type)) {
        activeCategories.delete(type);
        btn.classList.remove("active");
        btn.classList.add("off");
      } else {
        activeCategories.add(type);
        btn.classList.add("active");
        btn.classList.remove("off");
      }
      launchesOn = activeCategories.has("launches");
      applyFiltersLight();
      if (lastSnap) renderSnapshot(lastSnap);
    });
  });
}
initTargetsGrid();

let palette = {
  neptun: "#38e1ff",
  dimap: "#f5a623",
  shahmap: "#c084fc",
  radarua: "#f472b6",
  aviacontrol: "#ffea00",
  kupol: "#00e5ff",
  eradar: "#fb923c",
  ukrainealarm: "#10b981",
  consensus: "#7dff9a",
};
let consensusOn = true;
let lastSnap = null;

const ICON_TYPES = ["uav", "uav_jet", "fpv", "recon", "missile", "ballistic", "kab", "tactical", "mig31k", "unknown", "static"];
const TYPE_LABEL = {
  uav: "ШАХЕД-136", uav_jet: "РЕАКТИВНИЙ ШАХЕД", fpv: "FPV-ДРОН", recon: "РОЗВІДНИК", missile: "КР РАКЕТА",
  ballistic: "БАЛІСТИКА", kab: "КАБ", tactical: "ТАКТИЧНА АВІАЦІЯ", mig31k: "МІГ-31К", unknown: "НЕВІДОМЕ", static: "СТАТИЧНА МІТКА",
};
const typeOf = (t) => (TYPE_LABEL[t] ? t : "unknown");
const CONF_UA = { low: "низька", medium: "середня", high: "висока", unknown: "невідома" };

const TYPE_THEMES = {
  uav: {
    name: "Шахед-136",
    code: "SHD",
    primary: "#facc15",
    accent: "#eab308",
    glow: "#f59e0b",
    badgeBg: "rgba(250, 204, 21, 0.2)",
    badgeBorder: "#facc15",
    badgeColor: "#fde047",
    desc: "Поршневий ударний БпЛА (~180–200 км/год) · штовхаючий гвинт",
  },
  uav_jet: {
    name: "Шахед-238 (JET)",
    code: "JET",
    primary: "#ff5722",
    accent: "#ff3d00",
    glow: "#ff5722",
    badgeBg: "rgba(255, 87, 34, 0.25)",
    badgeBorder: "#ff5722",
    badgeColor: "#ff8a65",
    desc: "Турбореактивний ударний БпЛА (~450–600 км/год) · реактивний факел",
  },
  missile: {
    name: "Крилата ракета",
    code: "MSL",
    primary: "#ef4444",
    accent: "#dc2626",
    glow: "#ff2a4b",
    badgeBg: "rgba(239, 68, 68, 0.25)",
    badgeBorder: "#ef4444",
    badgeColor: "#f87171",
    desc: "Високоточна дозвукова ракета (~700–900 км/год) · розкривні крила",
  },
  ballistic: {
    name: "Балістична ракета",
    code: "BAL",
    primary: "#f43f5e",
    accent: "#c026d3",
    glow: "#ff007f",
    badgeBg: "rgba(244, 63, 94, 0.25)",
    badgeBorder: "#f43f5e",
    badgeColor: "#fb7185",
    desc: "Балістична / гіперзвукова траєкторія · надзвукові хвилі та ґратчасті керма",
  },
  kab: {
    name: "КАБ (авіабомба)",
    code: "KAB",
    primary: "#f97316",
    accent: "#ea580c",
    glow: "#ff8800",
    badgeBg: "rgba(249, 115, 22, 0.25)",
    badgeBorder: "#f97316",
    badgeColor: "#fb923c",
    desc: "Плануюча авіабомба з УМПК · характерне хвостове кільце та планерні крила",
  },
  fpv: {
    name: "FPV-дрон",
    code: "FPV",
    primary: "#00f5d4",
    accent: "#0d9488",
    glow: "#00e5ff",
    badgeBg: "rgba(0, 245, 212, 0.22)",
    badgeBorder: "#00f5d4",
    badgeColor: "#5eead4",
    desc: "Тактичний 4-роторний квадрокоптер · прямий відеоканал",
  },
  recon: {
    name: "БпЛА-розвідник",
    code: "RCN",
    primary: "#38bdf8",
    accent: "#0284c7",
    glow: "#0ea5e9",
    badgeBg: "rgba(56, 189, 248, 0.22)",
    badgeBorder: "#38bdf8",
    badgeColor: "#7dd3fc",
    desc: "Оперативно-тактичний коригувальник · наддовгі планерні крила та оптика",
  },
  tactical: {
    name: "Тактична авіація",
    code: "TAC",
    primary: "#818cf8",
    accent: "#6366f1",
    glow: "#4f46e5",
    badgeBg: "rgba(99, 102, 241, 0.25)",
    badgeBorder: "#818cf8",
    badgeColor: "#a5b4fc",
    desc: "Тактичний винищувач / бомбардувальник (Су-34, Су-35, Су-30) · носій КАБів та ракет Х-59/69",
  },
  mig31k: {
    name: "МіГ-31К",
    code: "MIG",
    primary: "#c084fc",
    accent: "#9333ea",
    glow: "#a855f7",
    badgeBg: "rgba(192, 132, 252, 0.25)",
    badgeBorder: "#c084fc",
    badgeColor: "#d8b4fe",
    desc: "Важкий надзвуковий перехоплювач · двокільовий хвіст та носій «Кинджала»",
  },
  unknown: {
    name: "Невідома ціль",
    code: "???",
    primary: "#fbbf24",
    accent: "#94a3b8",
    glow: "#e2e8f0",
    badgeBg: "rgba(251, 191, 36, 0.2)",
    badgeBorder: "#fbbf24",
    badgeColor: "#fde68a",
    desc: "Радарний контакт на доопрацюванні",
  },
};

// ==========================================================================
// Швидкості цілей та сектор розсіювання (Cone of Uncertainty)
// ==========================================================================
const THREAT_SPEED_KMH = {
  uav: 185,
  uav_jet: 500,
  missile: 800,
  ballistic: 3500,
  kab: 350,
  fpv: 90,
  recon: 120,
  tactical: 900,
  mig31k: 2200,
  unknown: 200,
};

function getProjectionParams(threatType) {
  const speed = THREAT_SPEED_KMH[threatType] || 200;
  let projMin = 30;
  if (threatType === "kab") projMin = 9;
  else if (threatType === "ballistic") projMin = 5;
  else if (threatType === "mig31k" || threatType === "tactical") projMin = 15;
  else if (threatType === "missile") projMin = 20;
  else if (threatType === "uav_jet") projMin = 25;
  else projMin = 30;

  const distKm = Math.min((speed * projMin) / 60, threatType === "kab" ? 50 : 400);
  return { speed, projMin, distKm };
}

function generateTargetCone(p, coords) {
  const heading =
    p.heading != null && p.heading !== "" && Number.isFinite(Number(p.heading))
      ? Number(p.heading)
      : null;
  if (heading == null) return null;

  const t = typeOf(p.rtype);
  const { speed, projMin, distKm } = getProjectionParams(t);
  const originLon = coords[0];
  const originLat = coords[1];
  const theme = TYPE_THEMES[t] || TYPE_THEMES.unknown;
  const color = p.color || theme.badgeBorder || "#38e1ff";

  // Сектор розсіювання ±12 градусів
  const spreadDeg = 12;
  const arcSteps = 16;
  const ring = [[originLon, originLat]];
  for (let i = 0; i <= arcSteps; i++) {
    const deg = heading - spreadDeg + (spreadDeg * 2 * i) / arcSteps;
    ring.push(geoDestPoint(originLat, originLon, deg, distKm));
  }
  ring.push([originLon, originLat]);

  const centerPt = geoDestPoint(originLat, originLon, heading, distKm);

  // Знаходимо населені пункти у конусі розсіювання
  const citiesOnCourse = [];
  for (const city of MAJOR_CITIES) {
    const d = geoHaversineKm(originLat, originLon, city.lat, city.lon);
    if (d < 5 || d > distKm) continue;
    const brg = geoBearingDeg(originLat, originLon, city.lat, city.lon);
    const diff = geoAngleDiff(brg, heading);
    if (diff <= spreadDeg) {
      const etaMin = Math.max(1, Math.round((d / speed) * 60));
      citiesOnCourse.push({
        name: city.name,
        distKm: Math.round(d),
        etaMin,
        region: city.region,
      });
    }
  }
  citiesOnCourse.sort((a, b) => a.distKm - b.distKm);

  const features = [
    {
      type: "Feature",
      geometry: { type: "Polygon", coordinates: [ring] },
      properties: { color, type: "cone-fill" },
    },
    {
      type: "Feature",
      geometry: { type: "LineString", coordinates: [[originLon, originLat], centerPt] },
      properties: { color, type: "cone-center" },
    },
  ];

  return { features, cities: citiesOnCourse, projMin, color, distKm };
}

function clearTargetCone() {
  const src = map.getSource("target-cone");
  if (src) {
    src.setData({ type: "FeatureCollection", features: [] });
  }
}

function isTypeActive(rtype) {
  const t = typeOf(rtype);
  if (t === "unknown") return false; // Повністю прибираємо невідомі цілі з карти
  if (t === "uav" || t === "uav_jet") return activeCategories.has("uav");
  if (t === "tactical" || t === "mig31k") return activeCategories.has("tactical");
  if (t === "missile") return activeCategories.has("missile");
  if (t === "ballistic") return activeCategories.has("ballistic");
  if (t === "kab") return activeCategories.has("kab");
  if (t === "recon" || t === "fpv") return activeCategories.has("recon_fpv");
  return false;
}

function isTargetUnconfirmed(p) {
  if (!p) return false;
  if (p.confirmed === true || p.confirmed === "true") return false;
  const isConfType = (p.confirmable === true || p.confirmable === "true") ||
    ["uav", "uav_jet", "missile", "ballistic"].includes(p.rtype);
  if (!isConfType) return false;
  const t = typeOf(p.rtype);
  if (t === "fpv" || t === "kab" || t === "tactical" || t === "mig31k" || t === "unknown") return false;
  return true;
}

// Той самий театр, що на бекенді.
const inTheater = (lat, lon) =>
  Number.isFinite(lat) && Number.isFinite(lon) &&
  lat >= 42 && lat <= 56 && lon >= 20 && lon <= 46;

// Прості, чіткі, монотонні тактичні гліфи (виразні за силуетом, без зайвих внутрішніх кольорів)
function drawSimpleIcon(ctx, type, isOutline, theme) {
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  const line = (x1, y1, x2, y2) => { ctx.beginPath(); ctx.moveTo(x1, y1); ctx.lineTo(x2, y2); ctx.stroke(); };
  const circle = (x, y, r) => { ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.stroke(); };
  const dot = (x, y, r) => { ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill(); };
  const poly = (pts) => {
    ctx.beginPath();
    pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
    ctx.closePath();
    ctx.fill();
    ctx.stroke();
  };

  switch (type) {
    case "uav": {
      // Shahed-136: монотонне широке дельта-крило, задній штовхаючий гвинт, законцівки
      poly([[48, 14], [84, 68], [76, 72], [54, 65], [48, 70], [42, 65], [20, 72], [12, 68]]);
      line(12, 62, 12, 74); // ліва законцівка (winglet)
      line(84, 62, 84, 74); // права законцівка (winglet)
      if (!isOutline) {
        line(42, 72, 54, 72); // штовхаючий гвинт
        line(48, 22, 48, 56); // осьовий кіль фюзеляжу
      }
      break;
    }
    case "uav_jet": {
      // Shahed-238: гостре реактивне дельта-крило + реактивне сопло
      poly([[48, 8], [78, 60], [70, 64], [52, 60], [44, 60], [26, 64], [18, 60]]);
      poly([[43, 61], [48, 88], [53, 61]]); // реактивний факел
      if (!isOutline) {
        line(48, 16, 48, 52); // гострий гребінь
      }
      break;
    }
    case "missile": {
      // Крилата ракета: тонкий корпус, розкривні прямі крила, керма
      poly([
        [48, 6], [51, 18], [51, 38], [86, 41], [86, 45], [51, 46],
        [51, 64], [64, 72], [51, 70], [45, 70], [32, 72], [45, 64],
        [45, 46], [10, 45], [10, 41], [45, 38], [45, 18]
      ]);
      poly([[46, 70], [48, 85], [50, 70]]); // факел ТРДД
      if (!isOutline) {
        line(48, 8, 48, 36);
      }
      break;
    }
    case "ballistic": {
      // Балістика: масивна ракета з гострим конусом, оперенням та дугою надзвукової хвилі
      poly([
        [48, 8], [55, 24], [55, 62], [70, 74], [54, 71],
        [42, 71], [26, 74], [41, 62], [41, 24]
      ]);
      ctx.beginPath();
      ctx.arc(48, 14, 16, Math.PI * 1.15, Math.PI * 1.85);
      ctx.stroke(); // ударна хвиля
      if (!isOutline) {
        line(48, 20, 48, 64);
        dot(48, 44, 3.5);
      }
      break;
    }
    case "kab": {
      // КАБ: краплеподібна бомба, планерні крила УМПК та хвостове кільце
      poly([
        [42, 18], [48, 10], [54, 18], [54, 30], [82, 42], [80, 46],
        [54, 37], [53, 62], [50, 72], [46, 72], [43, 62], [42, 37],
        [16, 46], [14, 42], [42, 30]
      ]);
      ctx.beginPath();
      ctx.ellipse(48, 72, 10, 4, 0, 0, Math.PI * 2);
      ctx.stroke(); // характерне кільце оперення
      if (!isOutline) {
        line(44, 22, 52, 22);
      }
      break;
    }
    case "fpv": {
      // Тактичний квадрокоптер: компактний тактичний дрон (~58% розміру від літаків / Шахедів)
      ctx.save();
      ctx.translate(48, 48);
      ctx.scale(0.58, 0.58);
      ctx.translate(-48, -48);
      ctx.lineWidth = ctx.lineWidth * 1.3;
      poly([
        [48, 20], [53, 27], [54, 40], [53, 56], [48, 64], [43, 56], [42, 40], [43, 27]
      ]);
      // 4 промені
      line(44, 32, 23, 19);
      line(52, 32, 73, 19);
      line(44, 52, 21, 67);
      line(52, 52, 75, 67);
      // 4 роторні диски
      circle(23, 19, 10.5);
      circle(73, 19, 10.5);
      circle(21, 67, 10.5);
      circle(75, 67, 10.5);
      if (!isOutline) {
        line(15, 19, 31, 19); // пропелери
        line(65, 19, 81, 19);
        line(13, 67, 29, 67);
        line(67, 67, 83, 67);
        dot(23, 19, 3.2); // маточини моторів
        dot(73, 19, 3.2);
        dot(21, 67, 3.2);
        dot(75, 67, 3.2);
        dot(48, 24, 2.8); // об'єктив камери
      }
      ctx.restore();
      break;
    }
    case "recon": {
      // Розвідник: планер з наддовгими крилами та Т-хвостом
      poly([[48, 10], [51, 16], [51, 72], [45, 72], [45, 16]]);
      poly([[45, 38], [6, 40], [6, 44], [45, 45], [51, 45], [90, 44], [90, 40], [51, 38]]);
      line(30, 72, 66, 72); // Т-подібний стабілізатор
      if (!isOutline) {
        dot(48, 42, 3.5); // камера розвідки
      }
      break;
    }
    case "tactical": {
      // Тактична авіація: винищувач (Су-34/35) з напливами LERX, стріловидними крилами, пілонами та хвостовим stinger
      poly([
        [48, 8], [50, 18], [51, 26], [55, 36], [84, 58], [84, 63], [56, 56],
        [56, 64], [72, 74], [68, 78], [54, 74], [52, 78], [48, 82], [44, 78],
        [42, 74], [28, 78], [24, 74], [40, 64], [40, 56], [12, 63], [12, 58],
        [41, 36], [45, 26], [46, 18]
      ]);
      line(42, 56, 39, 72); // лівий кіль
      line(54, 56, 57, 72); // правий кіль
      line(12, 56, 12, 64); // пілон лівого крила
      line(84, 56, 84, 64); // пілон правого крила
      if (!isOutline) {
        dot(48, 22, 3); // ліхтар кабіни
        line(48, 42, 48, 54);
      }
      break;
    }
    case "mig31k": {
      // МіГ-31К: перехоплювач зі стріловидними крилами, двома кілями та підвішеною ракетою Кинджал
      poly([
        [48, 6], [52, 18], [53, 34], [82, 60], [80, 64], [54, 58],
        [62, 72], [54, 70], [48, 66], [42, 70], [34, 72], [42, 58],
        [16, 64], [14, 60], [43, 34], [44, 18]
      ]);
      line(37, 56, 34, 72); // лівий кіль
      line(59, 56, 62, 72); // правий кіль
      if (!isOutline) {
        poly([[47, 34], [49, 34], [49, 52], [47, 52]]); // Кинджал під фюзеляжем
      }
      break;
    }
    case "static": {
      // Статична / непідтверджена засічка без вектора швидкості: круговий тактичний ретикул
      circle(48, 48, 25);
      circle(48, 48, 11);
      dot(48, 48, 4);
      line(48, 12, 48, 21);
      line(48, 75, 48, 84);
      line(12, 48, 21, 48);
      line(75, 48, 84, 48);
      break;
    }
    default: {
      // Невідоме: простий радарний ромб
      poly([[48, 16], [74, 48], [48, 80], [22, 48]]);
      if (!isOutline) {
        line(48, 24, 48, 72);
        line(28, 48, 68, 48);
      }
      break;
    }
  }
}

function makeIcon(type, color) {
  const c = document.createElement("canvas");
  c.width = 96;
  c.height = 96;
  const ctx = c.getContext("2d", { willReadFrequently: true });
  const col = color || "#ffffff";
  const theme = TYPE_THEMES[type] || TYPE_THEMES.unknown;

  // 1. Чорна контрастна підкладка / обводка
  ctx.save();
  ctx.shadowColor = "rgba(0,0,0,0.95)";
  ctx.shadowBlur = 10;
  ctx.fillStyle = "rgba(0,0,0,0.85)";
  ctx.strokeStyle = "#000000";
  ctx.lineWidth = 8;
  drawSimpleIcon(ctx, type, true, theme);
  ctx.restore();

  // 2. Кольоровий ореол джерела або консенсусу
  ctx.save();
  ctx.shadowColor = col;
  ctx.shadowBlur = 12;
  ctx.fillStyle = col;
  ctx.strokeStyle = col;
  ctx.lineWidth = 5;
  drawSimpleIcon(ctx, type, true, theme);
  ctx.restore();

  // 3. Простий, чіткий білий гліф з фірмовими акцентами типу
  ctx.save();
  ctx.fillStyle = "#ffffff";
  ctx.strokeStyle = "#ffffff";
  ctx.lineWidth = 3;
  drawSimpleIcon(ctx, type, false, theme);
  ctx.restore();

  return ctx.getImageData(0, 0, c.width, c.height);
}

function makeIcons() {
  for (const src of Object.keys(palette)) {
    for (const t of ICON_TYPES) {
      const id = `${t}-${src}`;
      if (!map.hasImage(id)) {
        try {
          map.addImage(id, makeIcon(t, palette[src]));
        } catch (e) {
          console.warn("icon failed", id, e);
        }
      } else {
        try {
          map.updateImage(id, makeIcon(t, palette[src]));
        } catch (e) {}
      }
    }
  }
}

function initLayers() {
  makeIcons();
  makeArrows();
  if (map.getSource("alerts")) return;
  const empty = { type: "FeatureCollection", features: [] };
  map.addSource("alerts", { type: "geojson", data: empty });
  map.addSource("launches", { type: "geojson", data: empty });
  map.addSource("corridors", { type: "geojson", data: empty });
  map.addSource("corridor-arrows", { type: "geojson", data: empty });
  map.addSource("launch-events", { type: "geojson", data: empty });
  map.addSource("targets", { type: "geojson", data: empty });
  map.addSource("threats", { type: "geojson", data: empty });
  map.addSource("clusters", { type: "geojson", data: empty });
  map.addSource("trails", { type: "geojson", data: empty });
  map.addSource("vectors", { type: "geojson", data: empty });
  map.addSource("target-cone", { type: "geojson", data: empty });
  // Сітка областей — зі стилю (шар obl), окремий geojson більше не тягнемо
  // Заливка тривог: червоний (ракети / загальна тривога) або жовтий (дронова загроза за ПКМУ №1092)
  map.addLayer({
    id: "alerts-fill", type: "fill", source: "alerts",
    paint: {
      "fill-color": [
        "case",
        ["==", ["get", "level"], "yellow"], "#ffd60a",
        ["==", ["get", "level"], "orange"], "#ff9f0a",
        "#ff3b30"
      ],
      "fill-opacity": [
        "case",
        ["==", ["get", "level"], "yellow"], 0.22,
        0.30
      ],
    },
  });
  map.addLayer({
    id: "alerts-line", type: "line", source: "alerts",
    paint: {
      "line-color": [
        "case",
        ["==", ["get", "level"], "yellow"], "#ffd60a",
        ["==", ["get", "level"], "orange"], "#ff9f0a",
        "#ff3b30"
      ],
      "line-opacity": 0.88,
      "line-width": [
        "case",
        ["==", ["get", "level"], "yellow"], 1.5,
        2.2
      ],
    },
  });

  // Статичні коридори пуск → кордон (монотонний спокійний пунктир, червоний при активному свіжому пуску)
  map.addLayer({
    id: "corridors", type: "line", source: "corridors",
    paint: {
      "line-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ff2222",
        "#475569"
      ],
      "line-width": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], 2.2,
        1.2
      ],
      "line-opacity": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], 0.9,
        0.35
      ],
      "line-dasharray": [2, 4],
    },
  });
  map.addLayer({
    id: "corridors-eta", type: "symbol", source: "corridors",
    layout: {
      "text-field": ["get", "eta_label"],
      "text-font": ["Noto Sans Bold"],
      "text-size": 10,
      "text-offset": [0, -0.9],
      "text-allow-overlap": true,
      "text-ignore-placement": true,
    },
    paint: {
      "text-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ffaaaa",
        "#94a3b8"
      ],
      "text-halo-color": "#000000",
      "text-halo-width": 1.5,
    },
  });
  // Стрілка прольоту: живе положення + таймер до кордону (рухається щосекунди).
  // Іконка і текст — окремі шари, щоб на смартфоні гасити тільки текст.
  map.addLayer({
    id: "corridor-arrows", type: "symbol", source: "corridor-arrows",
    layout: {
      "icon-image": ["get", "arrow"],
      "icon-size": 0.55,
      "icon-rotate": ["get", "bearing"],
      "icon-rotation-alignment": "map",
      "icon-allow-overlap": true,
      "icon-ignore-placement": true,
    },
  });
  map.addLayer({
    id: "corridor-arrows-timer", type: "symbol", source: "corridor-arrows",
    layout: {
      "text-field": ["get", "timer"],
      "text-font": ["Noto Sans Bold"],
      "text-size": 11,
      "text-offset": [0, 1.5],
      "text-allow-overlap": true,
      "text-ignore-placement": true,
    },
    paint: {
      "text-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ffaaaa",
        "#e2e8f0"
      ],
      "text-halo-color": "#000000",
      "text-halo-width": 1.5,
    },
  });

  map.addLayer({
    id: "trails", type: "line", source: "trails",
    paint: { "line-color": ["coalesce", ["get", "color"], "#94a3b8"], "line-width": 1.5, "line-opacity": 0.6 },
  });
  map.addLayer({
    id: "vectors", type: "line", source: "vectors",
    paint: {
      "line-color": ["coalesce", ["get", "color"], "#94a3b8"],
      "line-width": ["case", ["coalesce", ["get", "confirmed"], false], 2.5, 1.2],
      "line-opacity": ["case", ["coalesce", ["get", "confirmed"], false], 0.95, 0.45],
    },
  });

  // Сектор розсіювання (конус ймовірності курсу)
  map.addLayer({
    id: "target-cone-fill",
    type: "fill",
    source: "target-cone",
    filter: ["==", ["get", "type"], "cone-fill"],
    paint: {
      "fill-color": ["coalesce", ["get", "color"], "#38e1ff"],
      "fill-opacity": 0.16,
    },
  });
  map.addLayer({
    id: "target-cone-outline",
    type: "line",
    source: "target-cone",
    filter: ["==", ["get", "type"], "cone-fill"],
    paint: {
      "line-color": ["coalesce", ["get", "color"], "#38e1ff"],
      "line-width": 1.5,
      "line-dasharray": [3, 2],
      "line-opacity": 0.75,
    },
  });
  map.addLayer({
    id: "target-cone-center",
    type: "line",
    source: "target-cone",
    filter: ["==", ["get", "type"], "cone-center"],
    paint: {
      "line-color": ["coalesce", ["get", "color"], "#38e1ff"],
      "line-width": 2,
      "line-opacity": 0.85,
    },
  });

  // Гало навколо підтверджених консенсус-цілей (2+ джерела)
  map.addLayer({
    id: "targets-halo", type: "circle", source: "targets",
    filter: ["==", ["get", "confirmed"], true],
    paint: {
      "circle-color": "#7dff9a",
      "circle-radius": ["interpolate", ["linear"], ["zoom"], 4, 15, 8, 24, 12, 34],
      "circle-opacity": 0.32,
      "circle-blur": 1.2,
    },
  });
  // Тактичні іконки консолідованих цілей (крутяться за курсом)
  map.addLayer({
    id: "targets-icon", type: "symbol", source: "targets",
    layout: {
      "icon-image": ["get", "icon"],
      "icon-size": [
        "interpolate", ["linear"], ["zoom"],
        4, 0.38,
        8, 0.54,
        12, 0.72
      ],
      "icon-rotate": ["coalesce", ["get", "heading"], 0],
      "icon-rotation-alignment": "map",
      "icon-allow-overlap": true,
      "icon-ignore-placement": true,
        "symbol-sort-key": [
        "match", ["get", "rtype"],
        "ballistic", 0,
        "missile", 1,
        "mig31k", 2,
        "tactical", 3,
        "kab", 4,
        "uav_jet", 5,
        "uav", 6,
        "recon", 7,
        "fpv", 8,
        10
      ],
    },
    paint: {
      "icon-opacity": 1.0,
    },
  });
  // Бейдж кількості цілей (х2, х3 і більше — але не х1) над іконкою
  map.addLayer({
    id: "targets-badge", type: "symbol", source: "targets",
    filter: ["!=", ["get", "badge"], ""],
    layout: {
      "text-field": ["get", "badge"],
      "text-font": ["Noto Sans Bold"],
      "text-size": ["interpolate", ["linear"], ["zoom"], 4, 11, 8, 13, 12, 15],
      "text-offset": [0, -1.8],
      "text-allow-overlap": true,
      "text-ignore-placement": true,
    },
    paint: {
      "text-color": "#ffd166",
      "text-halo-color": "#000000",
      "text-halo-width": 2.5,
    },
  });
  map.addLayer({
    id: "threats", type: "symbol", source: "threats",
    layout: {
      "icon-image": ["get", "icon"],
      "icon-size": ["interpolate", ["linear"], ["zoom"], 4, 0.22, 8, 0.35, 12, 0.5],
      "icon-rotate": ["coalesce", ["get", "heading"], 0],
      "icon-rotation-alignment": "map",
      "icon-allow-overlap": false,
      "icon-ignore-placement": false,
      "symbol-sort-key": ["match", ["get", "rtype"],
        "ballistic", 0, "missile", 1, "mig31k", 1, "kab", 2,
        "recon", 4, "uav", 5, "fpv", 6, 9],
    },
    paint: {
      "icon-opacity": ["case", ["get", "area"], 0.45, 1],
    },
  });
  map.addLayer({
    id: "consensus", type: "circle", source: "clusters",
    paint: {
      "circle-color": ["coalesce", ["get", "color"], "#7dff9a"],
      "circle-radius": ["interpolate", ["linear"], ["get", "count"], 2, 11, 6, 17],
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 2,
    },
  });
  map.addLayer({
    id: "consensus-count", type: "symbol", source: "clusters",
    layout: {
      "text-field": ["to-string", ["get", "count"]],
      "text-font": ["Noto Sans Bold"],
      "text-size": 11, "text-allow-overlap": true,
    },
    paint: { "text-color": "#06130a" },
  });
  // Точки пусків: статичний довідковий шар (монотонний радарний стиль).
  // При свіжому пуску (<= 2 хв) стає червоним.
  map.addLayer({
    id: "launches", type: "circle", source: "launches",
    paint: {
      "circle-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ff1e1e",
        "#475569"
      ],
      "circle-radius": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], 7,
        5
      ],
      "circle-stroke-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ffffff",
        "#94a3b8"
      ],
      "circle-stroke-width": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], 2.5,
        1.5
      ],
      "circle-opacity": 0.95,
    },
  });
  map.addLayer({
    id: "launches-ring", type: "circle", source: "launches",
    paint: {
      "circle-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ff1e1e",
        "#475569"
      ],
      "circle-radius": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], 18,
        10
      ],
      "circle-opacity": 0,
      "circle-stroke-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ff4444",
        "#64748b"
      ],
      "circle-stroke-width": 1,
      "circle-stroke-opacity": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], 0.9,
        0.4
      ],
    },
  });
  map.addLayer({
    id: "launches-label", type: "symbol", source: "launches",
    layout: {
      "text-field": ["get", "name"],
      "text-font": ["Noto Sans Bold"],
      "text-size": 10,
      "text-offset": [0, 1.3],
      "text-anchor": "top",
      "text-allow-overlap": false,
      "text-ignore-placement": false,
    },
    paint: {
      "text-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ffffff",
        "#cbd5e1"
      ],
      "text-halo-color": "#000000",
      "text-halo-width": 1.5,
    },
  });
  // Активні пуски: тривожний маркер поверх точки пуску —
  // світне червоне гало при свіжому пуску
  map.addLayer({
    id: "launch-events-halo", type: "circle", source: "launch-events",
    paint: {
      "circle-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ff0000",
        "#64748b"
      ],
      "circle-radius": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], 28,
        18
      ],
      "circle-opacity": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], 0.35,
        0.15
      ],
      "circle-blur": 1.5,
    },
  });
  map.addLayer({
    id: "launch-events", type: "circle", source: "launch-events",
    paint: {
      "circle-color": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], "#ff1e1e",
        "#64748b"
      ],
      "circle-radius": [
        "case",
        ["coalesce", ["get", "is_fresh"], false], 14,
        10
      ],
      "circle-stroke-color": "#ffffff",
      "circle-stroke-width": 2,
      "circle-opacity": 0.8,
    },
  });

  // Точки пусків (довідково)
  map.on("click", "launches", (e) => {
    if (activeTargetPopup) { activeTargetPopup.remove(); activeTargetPopup = null; activeTargetId = null; }
    clearTargetCone();
    const p = e.features[0].properties;
    new maplibregl.Popup({ offset: 10 }).setLngLat(e.lngLat).setHTML(
      `<b>⌖ ПУСКОВА БАЗА ВОРОГА · ${p.name}</b><br>` +
      `Призначення: ${p.kind_label || p.kind}<br>` +
      `Координати: ${p.latlon}` +
      (p.note ? `<br><i>${p.note}</i>` : "") +
      `<br><span style="opacity:.6">Ворожа територія (РФ / ТОТ), OSINT ±10км</span>`
    ).addTo(map);
  });
  map.on("click", "launch-events", (e) => {
    if (activeTargetPopup) { activeTargetPopup.remove(); activeTargetPopup = null; activeTargetId = null; }
    clearTargetCone();
    const p = e.features[0].properties;
    const official = p.origin === "official";
    const originPlace = formatLaunchOrigin(p.name);
    new maplibregl.Popup({ offset: 12 }).setLngLat(e.lngLat).setHTML(
      (official ? `<b>▲ ВИПУЩЕНО З: ${originPlace}</b>` : `<b>⌖ ЙМОВІРНИЙ ПУСК З: ${originPlace}</b>`) +
      `<br><b>Тип:</b> ${p.kind_label || p.kind}<br>` +
      `<b>Район запуску (ворожа територія):</b> ${p.name}<br>` +
      `Бортів / пусків: ×${p.count} · ${Math.round(p.ago_min)} хв тому` +
      (official
        ? `<br><span style="opacity:.8;color:#7dff9a">✓ Офіційне сповіщення ПС ЗСУ</span>`
        : `<br><span style="opacity:.6">Авто-детект засічок з прикордоння РФ</span>`)
    ).addTo(map);
  });
  const onCorridorArrowClick = (e) => {
    const p = e.features[0].properties;
    new maplibregl.Popup({ offset: 12 }).setLngLat(e.lngLat).setHTML(
      `<b>➤ ПРОЛІТ · ${p.launch}</b><br>До України: <b>${p.timer}</b><br>` +
      `<span style="opacity:.6">Оцінка по крейсерській швидкості, не трек</span>`
    ).addTo(map);
  };
  map.on("click", "corridor-arrows", onCorridorArrowClick);
  map.on("click", "corridor-arrows-timer", onCorridorArrowClick);

  const onTargetClick = (e) => {
    // Запобігаємо подвійному спрацьовуванню в одному тапі (коли є одночасно іконка, бейдж і гало)
    if (e.originalEvent && e.originalEvent._handledTargetClick) return;
    if (e.originalEvent) e.originalEvent._handledTargetClick = true;

    if (!e.features || !e.features.length) return;
    const p = e.features[0].properties;
    const targetId = String(p.id || (p.rtype + "_" + (p.latlon || "")));
    const now = Date.now();

    // Якщо повторний клік по тій самій цілі — закриваємо попап (toggle: тицьнути ще раз щоб закрити)
    const isSameTarget = (activeTargetPopup && activeTargetId === targetId) ||
      (lastClosedTargetId === targetId && (now - lastClosedTime < 400));

    if (isSameTarget) {
      if (activeTargetPopup) {
        activeTargetPopup.remove();
        activeTargetPopup = null;
      }
      clearTargetCone();
      activeTargetId = null;
      lastClosedTargetId = null;
      return;
    }

    // Закриваємо попередній відкритий попап іншої цілі
    if (activeTargetPopup) {
      activeTargetPopup.remove();
      activeTargetPopup = null;
      activeTargetId = null;
      clearTargetCone();
    }

    const isConf = p.confirmed === true || p.confirmed === "true";
    const isConfirmable = p.confirmable === true || p.confirmable === "true" ||
      ["uav", "uav_jet", "missile", "ballistic"].includes(p.rtype);
    const srcCnt = Math.min(Math.max(p.source_count || 1, 1), 5);
    const srcCntLabel = srcCnt === 1 ? "1 радар" : (srcCnt <= 4 ? `${srcCnt} радари` : `${srcCnt} радарів`);
    const confLabel = !isConfirmable
      ? `<span style="color:#38e1ff;font-weight:600">⌖ ${(p.source || "RADAR").toUpperCase()} · ПРЯМИЙ МОНІТОРИНГ</span>`
      : (isConf
        ? `<span style="color:#7dff9a;font-weight:bold">✓ КОНСЕНСУС (підтверджено: ${srcCntLabel})</span>`
        : `<span style="color:#f5a623">◈ ОЧІКУЄ ПІДТВЕРДЖЕННЯ (${srcCntLabel} з 3)</span>`);
    const tgtCnt = Math.max(parseInt(p.target_count || 1, 10), 1);
    const isUav = ["uav", "uav_jet", "recon", "fpv"].includes(p.rtype);
    const unitName = isUav ? "БпЛА" : (p.rtype === "missile" || p.rtype === "ballistic" ? "ракет" : (p.rtype === "kab" ? "КАБ" : "бортів"));
    const countLabelHtml = (tgtCnt > 1)
      ? `<div style="margin:2px 0 4px 0;padding:2px 6px;background:rgba(255,209,102,0.12);border-left:2px solid #ffd166;border-radius:2px;font-size:11px"><span style="color:#ffd166;font-weight:bold">◈ КІЛЬКІСТЬ: група ×${tgtCnt} ${unitName}</span></div>`
      : `<div style="margin:2px 0 4px 0;font-size:11px;opacity:0.85"><span style="color:#cfc8b0">◈ КІЛЬКІСТЬ: 1 ${isUav ? "БпЛА (одиночна ціль)" : "одиниця"}</span></div>`;
    const hasHeading = (p.heading != null && p.heading !== "");
    const headingTxt = hasHeading
      ? `${Math.round(p.heading)}° (${bearingToLabel(p.heading)})`
      : `<span style="color:var(--yellow)">[ ! ] невідомий (статична засічка)</span>`;
    const staticWarningHtml = (!hasHeading && !isConf)
      ? `<div style="margin:4px 0 6px 0;padding:4px 8px;background:rgba(255,214,10,0.1);border-left:2px solid #ffd60a;border-radius:2px;font-size:11px;color:#ffd60a;line-height:1.3">[ ! ] <strong>Статична мітка без курсу</strong><br><span style="opacity:0.85">Відсутній радарний вектор швидкості. Ймовірний фантом або слуховий репорт.</span></div>`
      : "";
    const spreadTxt = (p.spread_km != null && p.spread_km > 0)
      ? `<br>Розкид між мапами: <b>${p.spread_km} км</b>`
      : "";
    const locTxt = p.locality ? `<br>Локація: <b>${p.locality}</b>` : (p.region ? `<br>Область: <b>${p.region}</b>` : "");

    const t = typeOf(p.rtype);
    const theme = TYPE_THEMES[t] || TYPE_THEMES.unknown;
    const typeBadges = {
      uav: {
        badge: "▲ ШАХЕД-136 (Shahed-136 / Герань-2)",
        desc: "Поршневий ударний БпЛА (~180–200 км/год) · штовхаючий гвинт",
      },
      uav_jet: {
        badge: "▶ РЕАКТИВНИЙ ШАХЕД (Shahed-238)",
        desc: "Турбореактивний ударний БпЛА (~450–600 км/год) · реактивний факел",
      },
      missile: {
        badge: `➤ КРИЛАТА РАКЕТА (${p.label || "Х-101 / Калібр"})`,
        desc: "Високоточна дозвукова ракета (~700–900 км/год) · низька висота",
      },
      ballistic: {
        badge: `⤉ БАЛІСТИЧНА РАКЕТА (${p.label || "Іскандер-М / Кинджал"})`,
        desc: "Балістична / гіперзвукова траєкторія · надзвукові хвилі",
      },
      kab: {
        badge: `▼ КЕРОВАНА АВІАБОМБА (${p.label || "КАБ з УМПК"})`,
        desc: "Плануюча авіабомба з УМПК · скид тактичної авіації",
      },
      fpv: {
        badge: "⌖ FPV-ДРОН (Квадрокоптер)",
        desc: "Тактичний дрон-камікадзе ближньої дії · 4 ротори",
      },
      recon: {
        badge: `◈ БпЛА-РОЗВІДНИК (${p.label || "Орлан-10 / ZALA / Supercam"})`,
        desc: "Оперативно-тактичний коригувальник · планерні крила та оптика",
      },
      mig31k: {
        badge: "⌲ МІГ-31К (Носій «Кинджала»)",
        desc: "Важкий надзвуковий перехоплювач · двокільовий хвіст",
      },
      tactical: {
        badge: `⌲ ТАКТИЧНА АВІАЦІЯ (${p.label || "Су-34 / Су-35 / Су-30"})`,
        desc: "Тактичний винищувач / бомбардувальник · пуски КАБ та ракет Х-59/69",
      },
      unknown: {
        badge: `◇ НЕВІДОМА ЦІЛЬ (${p.label || "Повітряний контакт"})`,
        desc: "Радарний контакт на доопрацюванні",
      },
    };
    const card = typeBadges[t] || typeBadges.unknown;
    const targetTitleHtml = `<div style="display:inline-block;padding:3px 8px;border-radius:4px;background:${theme.badgeBg};border:1px solid ${theme.badgeBorder};color:${theme.badgeColor};font-weight:700;font-size:11px;margin-bottom:4px">${card.badge}</div><br><span style="font-size:11px;color:${theme.badgeColor};opacity:0.95">${card.desc}</span>`;

    const membersHtml = String(p.members || "")
      .split("\n")
      .filter(Boolean)
      .map(s => `<div style="font-size:11px;opacity:0.85;padding-left:6px;border-left:2px solid #7dff9a;margin:2px 0;">${s}</div>`)
      .join("");

    const coords = (e.features[0].geometry && e.features[0].geometry.coordinates)
      ? e.features[0].geometry.coordinates.slice()
      : [e.lngLat.lng, e.lngLat.lat];

    let coneHtml = "";
    const coneData = generateTargetCone(p, coords);
    if (coneData) {
      const src = map.getSource("target-cone");
      if (src) {
        src.setData({ type: "FeatureCollection", features: coneData.features });
      }
      if (coneData.cities.length > 0) {
        const rows = coneData.cities.slice(0, 4).map((c) =>
          `<div class="cone-city-row">` +
          `<span class="cone-city-name"><b>${c.name}</b> <span class="cone-city-dist">(${c.distKm} км)</span></span>` +
          `<span class="cone-city-eta">~${c.etaMin} хв</span>` +
          `</div>`
        ).join("");
        coneHtml = `<div class="cone-cities-box" style="border-left-color:${coneData.color}">` +
          `<span class="cone-cities-title">⌖ Прогноз коридору (±12°, ${coneData.projMin} хв):</span>` +
          `${rows}` +
          `</div>`;
      } else {
        coneHtml = `<div class="cone-cities-box" style="border-left-color:${coneData.color}">` +
          `<span class="cone-cities-title">⌖ Прогноз коридору (±12°, ${coneData.projMin} хв):</span>` +
          `<span style="opacity:0.75;font-size:11px">За курсом немає великих міст у зоні ${coneData.projMin} хв</span>` +
          `</div>`;
      }
    } else {
      clearTargetCone();
    }

    const popup = new maplibregl.Popup({ offset: 14, closeButton: true, closeOnClick: true })
      .setLngLat(coords)
      .setHTML(
        `<div style="font-size:12px;line-height:1.4">` +
        `${targetTitleHtml}<br>` +
        `${confLabel}<br>` +
        `${countLabelHtml}` +
        `${staticWarningHtml}` +
        `Курс: <b>${headingTxt}</b>${spreadTxt}${locTxt}<br>` +
        `${coneHtml}` +
        `<span style="opacity:0.6">${p.latlon}</span>` +
        (membersHtml ? `<div style="margin-top:6px;border-top:1px solid #333;padding-top:4px"><b>Зведення засічок:</b><br>${membersHtml}</div>` : "") +
        `</div>`
      )
      .addTo(map);

    activeTargetPopup = popup;
    activeTargetId = targetId;

    popup.on("close", () => {
      clearTargetCone();
      lastClosedTargetId = targetId;
      lastClosedTime = Date.now();
      if (activeTargetPopup === popup) {
        activeTargetPopup = null;
        activeTargetId = null;
      }
    });
  };

  map.on("click", "targets-icon", onTargetClick);
  map.on("click", "targets-badge", onTargetClick);
  map.on("click", "targets-halo", onTargetClick);
  map.on("click", "threats", onTargetClick);
  map.on("click", "consensus", onTargetClick);
  for (const lid of ["targets-icon", "targets-badge", "threats"]) {
    map.on("mouseenter", lid, () => { map.getCanvas().style.cursor = "pointer"; });
    map.on("mouseleave", lid, () => { map.getCanvas().style.cursor = ""; });
  }
}

function bearingToLabel(b) {
  if (b == null) return "";
  const dirs = ["Північ", "Пн-Сх", "Схід", "Пд-Сх", "Південь", "Пд-Зх", "Захід", "Пн-Зх"];
  const idx = Math.round(((b % 360) / 45)) % 8;
  return dirs[idx];
}

function formatUsersCountUk(n) {
  const mod10 = n % 10;
  const mod100 = n % 100;
  if (mod100 >= 11 && mod100 <= 19) return `${n} користувачів`;
  if (mod10 === 1) return `${n} користувач`;
  if (mod10 >= 2 && mod10 <= 4) return `${n} користувачі`;
  return `${n} користувачів`;
}

function updateOnlineCount(cnt) {
  const badge = document.getElementById("online-badge");
  const el = document.getElementById("online-count");
  if (!el) return;
  const num = Math.max(1, Number(cnt) || 1);
  const formatted = num.toLocaleString("uk-UA");
  if (el.textContent !== formatted) {
    el.textContent = formatted;
    if (badge) {
      badge.classList.add("online-updated");
      setTimeout(() => badge.classList.remove("online-updated"), 800);
      badge.title = `${formatUsersCountUk(num)} переглядають тактичну карту онлайн у реальному часі`;
    }
  }
}

function renderSnapshot(snap) {
  initLayers();
  lastSnap = snap;
  if (snap.online_count != null) {
    updateOnlineCount(snap.online_count);
  }
  const rawFeats = snap.threats?.features || [];
  const rawTargets = snap.targets?.features || [];

  const junk = rawTargets.filter((f) => {
    const c = f.geometry?.coordinates || [];
    return !(Number.isFinite(c[0]) && Number.isFinite(c[1]) && inTheater(c[1], c[0]));
  }).length;
  if (junk) console.warn(`junk out-of-theater: ${junk}`);

  // Оновлюємо лічильники в тактичній сітці цілей
  let uavCount = 0;
  let uavJetCount = 0;
  let tacticalCount = 0;
  let migCount = 0;
  let missileCount = 0;
  let ballisticCount = 0;
  let kabCount = 0;
  let reconCount = 0;
  let unconfirmedCount = 0;
  for (const f of rawTargets) {
    const c = f.geometry?.coordinates || [];
    if (!inTheater(c[1], c[0])) continue;
    const p = f.properties || {};
    if (isTargetUnconfirmed(p)) {
      unconfirmedCount++;
    }
    const t = typeOf(p.rtype);
    if (t === "unknown") continue; // Повністю виключаємо невідомі цілі з БпЛА та авіації
    if (t === "uav_jet") {
      uavCount++;
      uavJetCount++;
    } else if (t === "uav") {
      uavCount++;
    } else if (t === "tactical") {
      tacticalCount++;
    } else if (t === "mig31k") {
      tacticalCount++;
      migCount++;
    } else if (t === "missile") missileCount++;
    else if (t === "ballistic") ballisticCount++;
    else if (t === "kab") kabCount++;
    else if (t === "recon" || t === "fpv") reconCount++;
  }
  const setCnt = (id, count) => {
    const el = document.getElementById(id);
    if (!el) return;
    el.textContent = count;
    if (count > 0 || (typeof count === "string" && count !== "0")) el.classList.add("has-targets");
    else el.classList.remove("has-targets");
  };
  let uavSub = [];
  if (uavJetCount > 0) uavSub.push(`JET:${uavJetCount}`);
  setCnt("cnt-uav", uavSub.length > 0 ? `${uavCount} (${uavSub.join(" ")})` : uavCount);
  let tacSub = [];
  if (migCount > 0) tacSub.push(`MIG:${migCount}`);
  setCnt("cnt-tactical", tacSub.length > 0 ? `${tacticalCount} (${tacSub.join(" ")})` : tacticalCount);
  setCnt("cnt-missile", missileCount);
  setCnt("cnt-ballistic", ballisticCount);
  setCnt("cnt-kab", kabCount);
  setCnt("cnt-recon", reconCount);
  const launchCount = (snap.launches?.features?.length || 19);
  setCnt("cnt-launches", launchCount);
  setCnt("cnt-unconfirmed", unconfirmedCount);

  const showUnconfirmed = activeCategories.has("unconfirmed");

  // Консолідовані цілі
  const targetFeats = rawTargets.filter((f) => {
    const c = f.geometry?.coordinates || [];
    if (!inTheater(c[1], c[0])) return false;
    if (!isTypeActive(f.properties.rtype)) return false;
    const p = f.properties;
    if (!showUnconfirmed && isTargetUnconfirmed(p)) return false;
    const hasHeading = p.heading != null && p.heading !== "";
    const t = typeOf(p.rtype);
    const s = p.confirmed ? "consensus" : (palette[p.source] ? p.source : "neptun");
    p.icon = `${t}-${s}`;
    return true;
  });

  // Сирі окремі засічки (для режиму без консолідації)
  const rawFiltered = rawFeats.filter((f) => {
    const c = f.geometry?.coordinates || [];
    if (!inTheater(c[1], c[0])) return false;
    if (!isTypeActive(f.properties.rtype)) return false;
    const p = f.properties;
    if (!showUnconfirmed && isTargetUnconfirmed(p)) return false;
    const hasHeading = p.heading != null && p.heading !== "";
    const t = typeOf(p.rtype);
    const s = palette[p.source] ? p.source : "neptun";
    p.icon = `${t}-${s}`;
    return true;
  });

  const filterLines = (fc) => {
    if (!fc || !fc.features) return { type: "FeatureCollection", features: [] };
    const feats = fc.features.filter((f) => {
      const p = f.properties || {};
      if (!isTypeActive(p.rtype)) return false;
      if (!showUnconfirmed && isTargetUnconfirmed(p)) return false;
      return true;
    });
    return { type: "FeatureCollection", features: feats };
  };

  if (consensusOn) {
    map.getSource("targets").setData({ type: "FeatureCollection", features: targetFeats });
    map.getSource("threats").setData({ type: "FeatureCollection", features: [] });
    map.getSource("clusters").setData({ type: "FeatureCollection", features: [] });
    map.getSource("trails").setData(filterLines(snap.trails));
    map.getSource("vectors").setData(filterLines(snap.vectors));
  } else {
    map.getSource("targets").setData({ type: "FeatureCollection", features: [] });
    map.getSource("threats").setData({ type: "FeatureCollection", features: rawFiltered });
    map.getSource("clusters").setData({ type: "FeatureCollection", features: [] });
    map.getSource("trails").setData(filterLines(snap.raw_trails || snap.trails));
    map.getSource("vectors").setData(filterLines(snap.raw_vectors || snap.vectors));
  }

  // Динамічна синхронізація конуса розсіювання для активної цілі
  if (activeTargetId) {
    const activeFeat = targetFeats.find((f) => {
      const p = f.properties || {};
      const tid = String(p.id || (p.rtype + "_" + (p.latlon || "")));
      return tid === activeTargetId;
    });
    if (activeFeat) {
      const coneData = generateTargetCone(activeFeat.properties, activeFeat.geometry.coordinates);
      if (coneData && map.getSource("target-cone")) {
        map.getSource("target-cone").setData({ type: "FeatureCollection", features: coneData.features });
      }
    } else {
      clearTargetCone();
    }
  }

  const total = targetFeats.length;
  const confirmed = targetFeats.filter((f) => f.properties.confirmed).length;
  const confirmableTargets = targetFeats.filter((f) =>
    f.properties.confirmable !== false &&
    f.properties.confirmable !== "false" &&
    ["uav", "uav_jet", "missile", "ballistic"].includes(f.properties.rtype)
  );
  const pending = Math.max(0, confirmableTargets.length - confirmed);
  const directTactical = Math.max(0, total - confirmableTargets.length);

  const counts = {};
  for (const f of rawFeats) {
    const s = f.properties.source || "neptun";
    counts[s] = (counts[s] || 0) + 1;
  }
  const order = ["neptun", "dimap", "shahmap", "radarua", "aviacontrol", "kupol", "eradar"];
  const parts = order.filter((s) => counts[s]).map((s) => `${s.toUpperCase()}:${counts[s]}`);
  for (const s of Object.keys(counts).sort()) {
    if (!order.includes(s)) parts.push(`${s.toUpperCase()}:${counts[s]}`);
  }

  const alertFeats = snap.alerts?.features || [];
  map.getSource("alerts").setData({ type: "FeatureCollection", features: alertFeats });

  if (snap.server_time) {
    const srv = Date.parse(snap.server_time) / 1000;
    if (Number.isFinite(srv)) clockOffsetSec = srv - Date.now() / 1000;
  }
  lastSnap = snap;
  lastEvents = snap.launch_events?.features || [];
  lastLaunches = snap.launches?.features || [];
  lastCorridors = snap.corridors?.features || [];

  const freshIds = updateFreshLaunches();
  refreshLaunchLayers(freshIds);

  const now = Date.now() / 1000 + clockOffsetSec;
  const eventFeats = lastEvents.map((f) => {
    const ep = f.properties;
    const ageSec = (ep.last_seen != null) ? Math.max(0, now - ep.last_seen) : ((ep.ago_min || 0) * 60);
    return {
      ...f,
      properties: {
        ...f.properties,
        is_fresh: ageSec <= LAUNCH_PULSE_SEC,
      },
    };
  });
  if (map.getSource("launch-events")) map.getSource("launch-events").setData({ type: "FeatureCollection", features: eventFeats });

  animateCorridorArrows();

  const countBadge = consensusOn
    ? `<span style="color:#ffffff;font-weight:700">ЦІЛЕЙ: ${total}</span> ` +
      `(підтверджено 3+: <span style="color:#7dff9a;font-weight:700">${confirmed}</span>` +
      (pending > 0 ? `, 1-2 дж.: <span style="color:#f5a623">${pending}</span>` : "") +
      (directTactical > 0 ? `, FPV/прямі: <span style="color:#38e1ff">${directTactical}</span>` : "") +
      (!showUnconfirmed && unconfirmedCount > 0 ? `, <span style="color:#f5a623;opacity:0.9">приховано: ${unconfirmedCount}</span>` : "") +
      `)`
    : `<span style="color:#ffffff;font-weight:700">СИРИХ ЗАСІЧОК: ${rawFiltered.length}</span>`;

  const redAlerts = alertFeats.filter((f) => (f.properties.level || "red").toLowerCase() !== "yellow").length;
  const yellowAlerts = alertFeats.filter((f) => (f.properties.level || "").toLowerCase() === "yellow").length;
  const alertBadge = yellowAlerts > 0
    ? `ТРИВОГИ: <span style="color:#ff3b30;font-weight:700">● ${redAlerts}</span> + <span style="color:#ffd60a;font-weight:700">● ${yellowAlerts}</span>`
    : (alertFeats.length > 0 ? `ТРИВОГ: <span style="color:#ff3b30;font-weight:700">● ${alertFeats.length}</span>` : "");

  document.getElementById("stats").innerHTML = alertBadge
    ? `${countBadge} · ${alertBadge}`
    : countBadge;
  applyFiltersLight();
}

// Пульсація міток активних пусків на карті: до 2 хвилин (120 с)
const LAUNCH_PULSE_SEC = 120;

// Час «тому» цокає ЖИВИМ на клієнті (серверний час + зсув годинника),
// а не замороженим значенням зі знімку.
function fmtAgo(sec) {
  if (!Number.isFinite(sec) || sec < 0) sec = 0;
  if (sec < 15) return "щойно";
  if (sec < 60) return `${Math.floor(sec)} с тому`;
  const m = Math.floor(sec / 60);
  const s = Math.floor(sec % 60);
  if (m < 2) return `${m} хв ${s} с тому`;
  if (m < 60) return `${m} хв тому`;
  return `${Math.floor(m / 60)} год ${m % 60} хв тому`;
}

function formatLaunchOrigin(name) {
  if (!name) return "ворожої території";
  const genMap = {
    "Курськ": "Курська",
    "Курськ (Халіно)": "Курська (Халіно)",
    "Воронеж": "Воронежа",
    "Бєлгород": "Бєлгорода",
    "Орел": "Орла",
    "Єйськ": "Єйська",
    "Моздок": "Моздока",
    "Шаталово": "Шаталово",
    "Міллерово": "Міллерово",
    "Приморсько-Ахтарськ": "Приморсько-Ахтарська",
    "Саваслейка": "Саваслейки",
    "Шайковка": "Шайковки",
    "Оленья": "Оленьї",
    "Енгельс-2": "Енгельса-2",
    "Дягілєво (Рязань)": "Дягілєво (Рязань)",
    "Джанкой (Крим)": "Джанкоя (Крим)",
    "Гвардійське (Крим)": "Гвардійського (Крим)",
    "Мис Чауда (Крим)": "Мису Чауда (Крим)",
    "Таганрог / Ростов": "Таганрога / Ростова",
    "Севастополь": "Севастополя",
    "Новоросійськ": "Новоросійська",
    "Каспій (акваторія)": "Каспію (акваторія)",
  };
  return genMap[name] || name;
}

function updateFreshLaunches() {
  const now = Date.now() / 1000 + clockOffsetSec;
  const freshSites = new Map();

  // 1. Події пусків (офіційні сповіщення ПС ЗСУ + детектор) - пульсують ДО 2 ХВИЛИН (120 с)
  for (const f of lastEvents) {
    const p = f.properties;
    const ageSec = (p.last_seen != null) ? Math.max(0, now - p.last_seen) : ((p.ago_min || 0) * 60);
    if (ageSec <= LAUNCH_PULSE_SEC && p.launch_id) {
      if (!freshSites.has(p.launch_id) || freshSites.get(p.launch_id).ageSec > ageSec) {
        freshSites.set(p.launch_id, {
          id: p.launch_id,
          name: p.name,
          kind: p.kind_label || p.kind,
          ageSec,
          count: p.count || 1,
          coords: f.geometry?.coordinates,
        });
      }
    }
  }

  // Пульсуючі маяки відображаються ВИКЛЮЧНО на ворожих точках пусків (lastEvents),
  // жодних позначок "ПУСК" всередині підконтрольної території України.

  // 2. Додавання / оновлення червоних пульсуючих маяків на карті (до 2 хв)
  const freshIds = new Set(freshSites.keys());
  for (const [id, info] of freshSites.entries()) {
    if (!info.coords && lastLaunches.length > 0) {
      const baseSite = lastLaunches.find((s) => s.properties?.id === id);
      if (baseSite?.geometry?.coordinates) {
        info.coords = baseSite.geometry.coordinates;
      }
    }
    if (!info.coords) continue;

    const originPlace = formatLaunchOrigin(info.name);
    const ageLabel = fmtAgo(info.ageSec);
    const actionLabel = info.kind === "mig31k" ? "ЗЛІТ" : "ПУСК";
    const tagText = `▲ ${actionLabel}: ${originPlace} · ${ageLabel}`;

    let entry = activePulseMarkers.get(id);
    if (!entry) {
      const el = document.createElement("div");
      el.className = "launch-pulse-beacon";
      el.innerHTML = `
        <div class="launch-pulse-wave"></div>
        <div class="launch-pulse-wave-delayed"></div>
        <div class="launch-pulse-center"></div>
        <div class="launch-pulse-tag">${tagText}</div>
      `;
      const marker = new maplibregl.Marker({ element: el })
        .setLngLat(info.coords)
        .addTo(map);
      activePulseMarkers.set(id, { marker, elTag: el.querySelector(".launch-pulse-tag"), info });
    } else {
      if (entry.elTag) {
        entry.elTag.textContent = tagText;
      }
    }
  }

  // 3. Видалення маяків, коли минуло > 2 хвилин (120 с)
  for (const [id, entry] of activePulseMarkers.entries()) {
    if (!freshIds.has(id)) {
      try {
        entry.marker.remove();
      } catch (e) {}
      activePulseMarkers.delete(id);
    }
  }

  // 5. Підсвітка кнопки «Бази / Пуски»
  const btnLaunches = document.querySelector('.grid-btn[data-type="launches"]');
  if (btnLaunches) {
    if (freshIds.size > 0) {
      btnLaunches.classList.add("has-fresh-launch");
    } else {
      btnLaunches.classList.remove("has-fresh-launch");
    }
  }

  return freshIds;
}

function refreshLaunchLayers(freshIds) {
  if (map.getSource("launches") && lastLaunches.length > 0) {
    const updated = lastLaunches.map((f) => ({
      ...f,
      properties: {
        ...f.properties,
        is_fresh: freshIds.has(f.properties.id),
      },
    }));
    map.getSource("launches").setData({ type: "FeatureCollection", features: updated });
  }
  if (map.getSource("corridors") && lastCorridors.length > 0) {
    const updated = lastCorridors.map((f) => ({
      ...f,
      properties: {
        ...f.properties,
        is_fresh: freshIds.has(f.properties.launch_id),
      },
    }));
    map.getSource("corridors").setData({ type: "FeatureCollection", features: updated });
  }
}

// --- Стрілки прольоту: одна на активний пуск, повзе пуск → кордон ---
// Положення = частка пролетілого ETA від first_seen (оцінка детектора),
// таймер = скільки лишилось до кордону. Цокає щосекунди разом із банером.
function havKm(lat1, lon1, lat2, lon2) {
  const R = 6371, d = Math.PI / 180;
  const a = Math.sin((lat2 - lat1) * d / 2) ** 2 +
    Math.cos(lat1 * d) * Math.cos(lat2 * d) * Math.sin((lon2 - lon1) * d / 2) ** 2;
  return 2 * R * Math.asin(Math.sqrt(a));
}

function bearingTo(lat1, lon1, lat2, lon2) {
  const d = Math.PI / 180;
  const y = Math.sin((lon2 - lon1) * d) * Math.cos(lat2 * d);
  const x = Math.cos(lat1 * d) * Math.sin(lat2 * d) -
    Math.sin(lat1 * d) * Math.cos(lat2 * d) * Math.cos((lon2 - lon1) * d);
  return (Math.atan2(y, x) / d + 360) % 360;
}

function fmtClock(sec) {
  if (!Number.isFinite(sec)) return "?";
  if (sec <= 0) return "≈ в Україні";
  sec = Math.floor(sec);
  const h = Math.floor(sec / 3600), m = Math.floor(sec % 3600 / 60), s = sec % 60;
  const mm = h ? String(m).padStart(2, "0") : String(m);
  const ss = String(s).padStart(2, "0");
  return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

// Точка на частці frac (0..1) довжини лінії [lon,lat][] → {lon, lat, bearing}
function pointAtFraction(line, frac) {
  if (line.length < 2) return null;
  const segs = [];
  let total = 0;
  for (let i = 0; i + 1 < line.length; i++) {
    const L = havKm(line[i][1], line[i][0], line[i + 1][1], line[i + 1][0]);
    segs.push(L);
    total += L;
  }
  if (!(total > 0)) return null;
  let target = Math.min(Math.max(frac, 0), 1) * total;
  for (let i = 0; i < segs.length; i++) {
    if (target <= segs[i] || i === segs.length - 1) {
      const t = segs[i] ? target / segs[i] : 0;
      const lon = line[i][0] + (line[i + 1][0] - line[i][0]) * t;
      const lat = line[i][1] + (line[i + 1][1] - line[i][1]) * t;
      return {
        lon, lat,
        bearing: bearingTo(line[i][1], line[i][0], line[i + 1][1], line[i + 1][0]),
      };
    }
    target -= segs[i];
  }
  return null;
}

function animateCorridorArrows() {
  if (!map.getSource("corridor-arrows")) return;
  const now = Date.now() / 1000 + clockOffsetSec;
  const feats = [];
  for (const ev of lastEvents) {
    const ep = ev.properties;
    if (ep.first_seen == null) continue;
    const corr = lastCorridors.find((f) => f.properties.launch_id === ep.launch_id);
    if (!corr) continue;
    const etaSec = (corr.properties.eta_min || 0) * 60;
    if (!(etaSec > 0)) continue;
    const elapsed = now - ep.first_seen;
    const frac = Math.min(Math.max(elapsed / etaSec, 0), 1);
    const g = corr.geometry;
    const lines = g.type === "MultiLineString" ? g.coordinates : [g.coordinates];
    // найдовша гілка — по ній і летимо
    let best = null, bestLen = -1;
    for (const ln of lines) {
      let L = 0;
      for (let i = 0; i + 1 < ln.length; i++) {
        L += havKm(ln[i][1], ln[i][0], ln[i + 1][1], ln[i + 1][0]);
      }
      if (L > bestLen) { bestLen = L; best = ln; }
    }
    const pos = best ? pointAtFraction(best, frac) : null;
    if (!pos) continue;
    const ageSec = (ep.last_seen != null) ? Math.max(0, now - ep.last_seen) : elapsed;
    const isFresh = ageSec <= LAUNCH_PULSE_SEC;
    const arrow = isFresh ? "arrow-alert" : "arrow-mono";
    feats.push({
      type: "Feature",
      geometry: { type: "Point", coordinates: [pos.lon, pos.lat] },
      properties: {
        arrow,
        is_fresh: isFresh,
        bearing: Math.round(pos.bearing * 10) / 10,
        timer: fmtClock(etaSec - elapsed),
        launch: ep.name || corr.properties.launch,
        color: isFresh ? "#ff2222" : "#94a3b8",
      },
    });
  }
  map.getSource("corridor-arrows").setData({ type: "FeatureCollection", features: feats });
}

setInterval(() => {
  const freshIds = updateFreshLaunches();
  refreshLaunchLayers(freshIds);
  animateCorridorArrows();
}, 1000);

function applyFiltersLight() {
  if (!map.getLayer("launches")) return;
  const vis = launchesOn ? "visible" : "none";
  for (const id of ["launches", "launches-ring", "launches-label",
    "corridors", "corridors-eta", "corridor-arrows", "corridor-arrows-timer"]) {
    if (map.getLayer(id)) map.setLayoutProperty(id, "visibility", vis);
  }
  // launch-events видно завжди — це тривожний шар, керується наявністю подій
  applyMobileText();
}

// --- Мобільний режим: на вузькому екрані гасимо текстові шари (мітки лишаються)
const mqMobile = matchMedia("(max-width: 720px)");
function applyMobileText() {
  if (!map.getLayer("launches")) return;
  const textVis = (!mqMobile.matches && launchesOn) ? "visible" : "none";
  for (const id of ["launches-label", "corridors-eta", "corridor-arrows-timer"]) {
    if (map.getLayer(id)) map.setLayoutProperty(id, "visibility", textVis);
  }
}
if (mqMobile.addEventListener) mqMobile.addEventListener("change", applyMobileText);

document.getElementById("hud-toggle").addEventListener("click", () => {
  document.body.classList.toggle("panels-hidden");
});

async function loadMeta() {
  try {
    const h = await fetch(`${API}/api/health`).then((r) => r.json());
    document.getElementById("ver").textContent = h.version || "v?";
  } catch {}
  try {
    const arr = await fetch(`${API}/api/sources`).then((r) => r.json());
    for (const s of arr) {
      if (s.color) palette[s.name] = s.color;
    }
  } catch (e) {
    console.warn("sources failed, default palette", e);
  }
  try {
    makeIcons();
  } catch (e) {
    console.warn("icons failed", e);
  }
  try {
    makeArrows();
  } catch (e) {
    console.warn("arrows failed", e);
  }
}

async function pollRest() {
  try {
    const r = await fetch(`${API}/api/geo`);
    if (r.ok) renderSnapshot(await r.json());
  } catch {
    document.getElementById("stats").textContent = "НЕМАЄ ЗВ'ЯЗКУ — перевір бекенд";
  }
}

function connectWs() {
  try {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/ws/geo`);
    ws.onmessage = (e) => {
      try {
        const data = JSON.parse(e.data);
        if (data.type === "online") {
          updateOnlineCount(data.online_count);
        } else {
          renderSnapshot(data);
        }
      } catch {}
    };
    ws.onclose = () => setTimeout(connectWs, 3000);
    ws.onerror = () => ws.close();
  } catch {
    setInterval(pollRest, 5000);
  }
}

map.on("load", async () => {
  // На смартфоні стартуємо зі складеними панелями і без текстів
  if (mqMobile.matches) document.body.classList.add("panels-hidden");
  // Опитування стартує ЗАВЖДИ, навіть якщо мета/стрілки впали
  try { await loadMeta(); } catch (e) { console.warn("meta failed", e); }
  pollRest(); connectWs(); setInterval(pollRest, 15000);
});

// --- Модальне вікно оцінки точності карт ---
let accuracyPeriod = "1h";
let accuracyType = "all";
let showTransparency = false;

async function loadAccuracyData() {
  const container = document.getElementById("accuracy-leaderboard");
  const loading = document.getElementById("accuracy-loading");
  if (!container) return;
  if (loading) loading.hidden = false;

  try {
    const res = await fetch(`${API}/api/stats/accuracy?period=${accuracyPeriod}&type=${accuracyType}`);
    if (!res.ok) throw new Error("API error " + res.status);
    const data = await res.json();
    if (loading) loading.hidden = true;
    renderLeaderboard(data);
  } catch (err) {
    if (loading) loading.hidden = true;
    container.innerHTML = `<div style="color:var(--red);padding:10px;font-size:11px">Помилка завантаження даних: ${err.message}</div>`;
  }
}

function renderLeaderboard(data) {
  const container = document.getElementById("accuracy-leaderboard");
  if (!container) return;

  if (showTransparency) {
    container.classList.add("show-transparent");
  } else {
    container.classList.remove("show-transparent");
  }

  const btnTransp = document.getElementById("btn-toggle-transparency");
  const txtTransp = document.getElementById("transparency-btn-text");
  if (btnTransp && txtTransp) {
    if (showTransparency) {
      btnTransp.classList.add("active");
      txtTransp.textContent = "ПРИХОВАТИ ПРОЗОРІ ДАНІ (ЗГОРНУТИ)";
    } else {
      btnTransp.classList.remove("active");
      txtTransp.textContent = "ПОКАЗАТИ ПРОЗОРІ ДАНІ ТА ШТРАФИ";
    }
  }

  if (!data.sources || data.sources.length === 0) {
    container.innerHTML = `<div style="color:var(--khaki-dim);padding:10px;font-size:11px">Дані за цей період ще накопичуються…</div>`;
    return;
  }

  const rankBadges = ["[ 01 ]", "[ 02 ]", "[ 03 ]", "[ 04 ]", "[ 05 ]"];
  const offBanner = (data.total_official_notices && data.total_official_notices > 0)
    ? `<span class="summary-sep">·</span><span class="summary-item" title="Накопичено офіційних повідомлень Повітряних Сил ЗСУ">[ ✓ ПС ЗСУ: <strong>${data.total_official_notices} зведень</strong> ]</span>`
    : "";

  let html = `
    <div class="accuracy-summary-banner">
      <span class="summary-item">◈ ПІДТВЕРДЖЕНО В НЕБІ: <strong>${data.total_confirmed_clusters}</strong></span>
      <span class="summary-sep">·</span>
      <span class="summary-item">⌖ НАЙВИЩА ДОСТОВІРНІСТЬ: <strong style="color:var(--green)">${(data.leader || "—").toUpperCase()}</strong></span>
      ${offBanner}
    </div>
  `;

  for (const s of data.sources) {
    const rankText = rankBadges[s.rank - 1] || `[ ${s.rank < 10 ? '0' + s.rank : s.rank} ]`;
    const color = palette[s.source] || "#e3d199";
    const isFirst = s.rank === 1;

    const offPct = s.official_concordance_pct != null ? s.official_concordance_pct : 100;
    const offText = (s.official_total && s.official_total > 0)
      ? `${offPct}% (${s.official_matches}/${s.official_total})`
      : `${s.confirmed_detected}/${s.total_reports}`;

    const earnedFmt = (s.earned_points || 0).toLocaleString();
    const penaltyFmt = (s.penalty_points || 0).toLocaleString();
    const netFmt = (s.net_points || 0).toLocaleString();
    const maxFmt = (s.max_possible_points || 0).toLocaleString();
    const noAlertViol = s.no_alert_violations || 0;
    const staticViol = s.static_phantom_violations || 0;

    html += `
      <div class="source-card rank-${s.rank} ${isFirst ? "rank-1" : ""}" id="card-${s.source}">
        <div class="source-card-header">
          <div class="source-title-group">
            <span class="source-rank-badge rank-badge-${s.rank}">${rankText}</span>
            <span class="dot" style="background:${color};border-radius:2px"></span>
            <span class="source-name-badge" style="color:${color}">${s.source.toUpperCase()}</span>
          </div>
          <div class="source-score-badge">
            ${s.score}%
          </div>
        </div>
        <div class="progress-track">
          <div class="progress-fill" style="width:${s.score}%;background:${color}"></div>
        </div>
        <div class="source-metrics-grid">
          <div class="metric-box" title="Правдивість: частка підтверджених цілей серед усіх виданих картою">
            <span class="metric-lbl">⌖ Правдивість</span>
            <span class="metric-val">${s.precision}%</span>
          </div>
          <div class="metric-box" title="Уважність: відсоток справжніх дронів у небі, які помітила ця карта">
            <span class="metric-lbl">◉ Уважність</span>
            <span class="metric-val">${s.recall}%</span>
          </div>
          <div class="metric-box" title="Похибка: середня відстань у км від центру консенсусу">
            <span class="metric-lbl">⊕ Похибка</span>
            <span class="metric-val">${s.avg_spatial_error_km} км</span>
          </div>
          <div class="metric-box" title="Офіційна відповідність: скільки офіційних сповіщень ПС ЗСУ підтвердила ця карта">
            <span class="metric-lbl">✓ Збіг з ПС ЗСУ</span>
            <span class="metric-val">${offText}</span>
          </div>
        </div>

        <button type="button" class="source-card-toggle" data-source="${s.source}">
          <span class="toggle-icon">▾</span> Прозорі розрахунки та штрафи
        </button>

        <div class="source-card-details">
          <div class="calc-formula-panel">
            <div class="calc-formula-header">
              <span>Формула балів доби: Net / Max</span>
              <span class="calc-formula-res">${s.score}%</span>
            </div>
            <div class="calc-formula-expr">
              (${netFmt} чистих / ${maxFmt} макс) × 100%
            </div>
          </div>

          <div class="details-subgrid">
            <div class="details-tile">
              <span class="details-tile-lbl">Зароблені бали</span>
              <span class="details-tile-val pos">+${earnedFmt} б.</span>
              <span class="details-tile-desc">100 б./борт + бонус точності</span>
            </div>
            <div class="details-tile">
              <span class="details-tile-lbl">Штрафні бали</span>
              <span class="details-tile-val ${s.penalty_points > 0 ? "neg" : ""}">${s.penalty_points > 0 ? '-' + penaltyFmt + ' б.' : '0 б.'}</span>
              <span class="details-tile-desc">Списано за порушення</span>
            </div>
            <div class="details-tile">
              <span class="details-tile-lbl">Підтверджено бортів</span>
              <span class="details-tile-val">${s.confirmed_detected} / ${data.total_confirmed_clusters}</span>
              <span class="details-tile-desc">Реальних дронів консенсусу</span>
            </div>
            <div class="details-tile">
              <span class="details-tile-lbl">Всього репортів</span>
              <span class="details-tile-val">${s.total_reports}</span>
              <span class="details-tile-desc">Видано цим джерелом</span>
            </div>
            <div class="details-tile">
              <span class="details-tile-lbl">БпЛА без тривоги</span>
              <span class="details-tile-val ${noAlertViol > 0 ? "neg" : ""}">${noAlertViol} інцид.</span>
              <span class="details-tile-desc">-30 б./випадок (кулдаун 10 хв)</span>
            </div>
            <div class="details-tile">
              <span class="details-tile-lbl">Завислі фантоми</span>
              <span class="details-tile-val ${staticViol > 0 ? "neg" : ""}">${staticViol} інцид.</span>
              <span class="details-tile-desc">-15 б./випадок (без курсу)</span>
            </div>
            <div class="details-tile">
              <span class="details-tile-lbl">Геодезичний бонус</span>
              <span class="details-tile-val">${s.spatial_score}%</span>
              <span class="details-tile-desc">Сер. похибка ±${s.avg_spatial_error_km} км</span>
            </div>
            <div class="details-tile">
              <span class="details-tile-lbl">Стабільність</span>
              <span class="details-tile-val">${s.stability_score}%</span>
              <span class="details-tile-desc">Без аномалій та стрибків</span>
            </div>
          </div>

          <div class="details-footer-row">
            <span style="font-size:9.5px;color:var(--khaki-dim)">Чисті бали: <strong style="color:var(--khaki)">${netFmt}</strong></span>
            <button type="button" class="copy-json-btn" data-source="${s.source}">⎘ СКОПІЮВАТИ JSON ДЖЕРЕЛА</button>
          </div>
        </div>
      </div>
    `;
  }

  container.innerHTML = html;

  // Інтерактивні кліки по кнопках розгортання окремих карток
  container.querySelectorAll(".source-card-toggle").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const card = btn.closest(".source-card");
      if (!card) return;
      card.classList.toggle("expanded");
      const isExp = card.classList.contains("expanded");
      btn.innerHTML = isExp
        ? `<span class="toggle-icon">▴</span> Приховати прозорі дані`
        : `<span class="toggle-icon">▾</span> Прозорі розрахунки та штрафи`;
    });
  });

  // Кнопка копіювання JSON джерела
  container.querySelectorAll(".copy-json-btn").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const srcName = btn.getAttribute("data-source");
      const sObj = data.sources.find((x) => x.source === srcName);
      if (sObj && navigator.clipboard) {
        navigator.clipboard.writeText(JSON.stringify(sObj, null, 2)).then(() => {
          const old = btn.textContent;
          btn.textContent = "✓ Скопійовано!";
          setTimeout(() => { btn.textContent = old; }, 1500);
        });
      }
    });
  });
}

function initAccuracyUI() {
  const btnAcc = document.getElementById("btn-accuracy");
  const modal = document.getElementById("accuracy-modal");
  const btnClose = document.getElementById("modal-close");
  const btnTransp = document.getElementById("btn-toggle-transparency");

  if (btnAcc && modal) {
    btnAcc.addEventListener("click", () => {
      modal.hidden = false;
      loadAccuracyData();
    });
  }
  if (btnClose && modal) {
    btnClose.addEventListener("click", () => {
      modal.hidden = true;
    });
  }
  if (modal) {
    modal.addEventListener("click", (e) => {
      if (e.target === modal) modal.hidden = true;
    });
  }

  if (btnTransp) {
    btnTransp.addEventListener("click", () => {
      showTransparency = !showTransparency;
      const container = document.getElementById("accuracy-leaderboard");
      const txt = document.getElementById("transparency-btn-text");
      if (showTransparency) {
        btnTransp.classList.add("active");
        if (txt) txt.textContent = "ПРИХОВАТИ ПРОЗОРІ ДАНІ (ЗГОРНУТИ)";
        if (container) container.classList.add("show-transparent");
      } else {
        btnTransp.classList.remove("active");
        if (txt) txt.textContent = "ПОКАЗАТИ ПРОЗОРІ ДАНІ ТА ШТРАФИ";
        if (container) container.classList.remove("show-transparent");
      }
    });
  }

  document.querySelectorAll(".period-pills button").forEach((b) => {
    b.addEventListener("click", () => {
      document.querySelectorAll(".period-pills button").forEach((x) => x.classList.remove("active"));
      b.classList.add("active");
      accuracyPeriod = b.getAttribute("data-period");
      loadAccuracyData();
    });
  });

  document.querySelectorAll(".type-pills button").forEach((b) => {
    b.addEventListener("click", () => {
      document.querySelectorAll(".type-pills button").forEach((x) => x.classList.remove("active"));
      b.classList.add("active");
      accuracyType = b.getAttribute("data-type");
      loadAccuracyData();
    });
  });
}
initAccuracyUI();

function initTosUI() {
  const btnTos = document.getElementById("btn-tos");
  const btnDisclaimerTos = document.getElementById("btn-disclaimer-tos");
  const disclaimer = document.getElementById("disclaimer");
  const modal = document.getElementById("tos-modal");
  const btnClose = document.getElementById("tos-modal-close");

  const openTos = () => {
    if (modal) modal.hidden = false;
  };
  const closeTos = () => {
    if (modal) modal.hidden = true;
  };

  if (btnTos) btnTos.addEventListener("click", openTos);
  if (btnDisclaimerTos) btnDisclaimerTos.addEventListener("click", openTos);
  if (disclaimer) disclaimer.addEventListener("click", openTos);
  if (btnClose) btnClose.addEventListener("click", closeTos);
  if (modal) {
    modal.addEventListener("click", (e) => {
      if (e.target === modal) closeTos();
    });
  }
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && modal && !modal.hidden) closeTos();
  });
}
initTosUI();

