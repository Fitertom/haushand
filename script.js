/* =========================================================
   КОНФИГ — поменяй здесь номер и т.д.
   ========================================================= */
const CONFIG = {
  phone: "+4915100000000",          // номер в международном формате без пробелов
  phoneDisplay: "+49 151 0000 0000",
  whatsapp: "4915100000000",        // для wa.me — только цифры
};

/* =========================================================
   ГОРОДА: определяем по IP. Можно задать вручную в ссылке: ?city=essen
   ========================================================= */
const CITIES = {
  essen:      { name: "Essen",               short: "Essen" },
  duisburg:   { name: "Duisburg",            short: "Duisburg" },
  oberhausen: { name: "Oberhausen",          short: "Oberhausen" },
  muelheim:   { name: "Mülheim an der Ruhr", short: "Mülheim" },
};
const ALL_CITIES = "Essen · Duisburg · Oberhausen · Mülheim";

/* =========================================================
   УСЛУГИ (порядок = порядок на сайте)
   ========================================================= */
const SERVICES = [
  { id: "sofa",      cat: "clean", icon: "s-sofa",
    title: "Polsterreinigung", desc: "Sofa, Sessel, Matratzen und Stühle – Tiefenreinigung direkt bei Ihnen vor Ort." },
  { id: "furniture", cat: "home",  icon: "s-furniture",
    title: "Möbelmontage", desc: "Schränke, Betten, Küchenmodule, Regale – IKEA & Co. schnell und stabil aufgebaut." },
  { id: "mount",     cat: "home",  icon: "s-drill",
    title: "Bohren & Aufhängen", desc: "Regale, Bilder, Spiegel, Gardinenstangen, TV-Halterungen fachgerecht montieren." },
  { id: "windows",   cat: "clean", icon: "s-window",
    title: "Fensterreinigung", desc: "Streifenfreie Fenster inkl. Rahmen und Fensterbänke – privat und für Büros." },
  { id: "moveout",   cat: "clean", icon: "s-clean",
    title: "Endreinigung", desc: "Gründliche Reinigung nach Umzug oder Renovierung – bereit für die Übergabe." },
  { id: "repair",    cat: "home",  icon: "s-repair",
    title: "Kleinreparaturen", desc: "Türen und Schränke einstellen, Silikonfugen erneuern, Griffe tauschen u. v. m." },
  { id: "paint",     cat: "home",  icon: "s-paint",
    title: "Wände streichen", desc: "Wände und Decken streichen, abkleben, abdecken – ideal vor Einzug oder Auszug." },
  { id: "wallpaper", cat: "home",  icon: "s-wallpaper",
    title: "Tapezieren", desc: "Raufaser, Vlies- und Mustertapeten sauber tapezieren, alte Tapete entfernen." },
  { id: "tech",      cat: "extra", icon: "s-wifi",
    title: "Technik-Hilfe", desc: "WLAN einrichten, PC reinigen, Drucker oder Smart-TV anschließen und mehr." },
];

const FAQ = [
  ["Wie schnell können Sie kommen?", "In der Regel innerhalb von 1–3 Tagen, bei kleinen Aufträgen oft schon am nächsten Tag."],
  ["Muss ich Material besorgen?", "Nein, das müssen Sie nicht. Farbe, Tapete oder Dübel können wir auf Wunsch mitbringen und zum Einkaufspreis berechnen."],
  ["Wie wird bezahlt?", "Nach getaner Arbeit – bar, per Überweisung oder PayPal. Sie erhalten immer eine Rechnung."],
  ["Welche Arbeiten übernehmen Sie nicht?", "Elektro-, Gas- und Wasserinstallationen sowie meisterpflichtige Handwerksarbeiten. Dafür empfehlen wir Ihnen gern einen Fachbetrieb."],
  ["Was kostet der Einsatz?", "Das hängt vom Umfang ab. Rufen Sie uns an oder schicken Sie Fotos – wir nennen Ihnen den Preis, bevor wir anfangen. Ohne Überraschungen."],
  ["Kann ich Fotos schicken?", "Ja, bitte! Per WhatsApp können wir den Aufwand am besten einschätzen."],
];

/* ---------- Лендинг под объявление: ?s=sofa | furniture | mount | paint ... ---------- */
const params = new URLSearchParams(location.search);
const LANDING = params.get("s");
const landingService = SERVICES.find((x) => x.id === LANDING);

let city = null; // ключ из CITIES или null (не определили)

/* =========================================================
   РЕНДЕР
   ========================================================= */
function waLink(text) {
  return `https://wa.me/${CONFIG.whatsapp}?text=${encodeURIComponent(text)}`;
}

function renderServices() {
  document.getElementById("servicesGrid").innerHTML = SERVICES.map((s) => `
      <article class="card card--${s.cat}${s.id === LANDING ? " is-featured" : ""}" data-cat="${s.cat}">
        <div class="card__icon"><svg><use href="#${s.icon}"/></svg></div>
        <h3>${s.title}</h3>
        <p>${s.desc}</p>
        <div class="card__foot">
          <a class="card__link" href="${waLink(`Hallo! Ich interessiere mich für: ${s.title}`)}" target="_blank" rel="noopener">Anfragen →</a>
        </div>
      </article>`).join("");
}

function renderFaq() {
  document.getElementById("faqList").innerHTML = FAQ
    .map(([q, a]) => `<details><summary>${q}</summary><p>${a}</p></details>`).join("");
}

function renderFormOptions() {
  const sel = document.getElementById("formService");
  sel.innerHTML = SERVICES.map((s) => `<option value="${s.id}">${s.title}</option>`).join("")
    + `<option value="other">Sonstiges</option>`;
  if (landingService) sel.value = landingService.id;
}

function renderContacts() {
  document.querySelectorAll(".js-wa").forEach((a) => (a.href = waLink("Hallo! Ich habe eine Anfrage.")));
  document.querySelectorAll(".js-tel").forEach((a) => (a.href = `tel:${CONFIG.phone}`));
  document.querySelectorAll(".js-phone-text").forEach((el) => (el.textContent = CONFIG.phoneDisplay));
}

/* Всё, что зависит от города */
function renderCity() {
  const c = CITIES[city];
  const $ = (id) => document.getElementById(id);

  $("heroBadge").textContent = c ? `${c.name} · Termine oft schon morgen` : `Immer in Ihrer Nähe · ${ALL_CITIES}`;

  if (landingService) {
    $("heroTitle").innerHTML = `${landingService.title} ${c ? `in ${c.short}` : "in Ihrer Nähe"} <span class="hl">schnell & zuverlässig</span>`;
    $("heroLead").textContent = `${landingService.desc} Rufen Sie einfach an – den Preis besprechen wir direkt mit Ihnen. Termine oft schon morgen.`;
  } else {
    $("heroTitle").innerHTML = `Hilfe im Haushalt${c ? ` in ${c.short}` : ""}? <span class="hl">Wir erledigen das.</span>`;
  }

  $("areaTitle").textContent = c ? `Einsatzgebiet: ${c.name} und Umgebung` : "Wir sind immer in Ihrer Nähe";
  $("contactArea").textContent = c ? `${c.name} und Umgebung` : ALL_CITIES;
  document.querySelectorAll("#areaChips li").forEach((li) => li.classList.toggle("chip--alt", li.dataset.city === city));

  document.title = `HausHand ${c ? c.short : "Ruhrgebiet"} – Polsterreinigung, Möbelmontage & mehr`;
}

/* =========================================================
   ОПРЕДЕЛЕНИЕ ГОРОДА ПО IP
   ========================================================= */
function matchCity(raw, countryCode) {
  if (!raw || (countryCode && countryCode.toUpperCase() !== "DE")) return null;
  const n = String(raw).toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/ue/g, "u");
  if (n.startsWith("essen")) return "essen";
  if (n.startsWith("duisburg")) return "duisburg";
  if (n.startsWith("oberhausen")) return "oberhausen";
  if (n.startsWith("mulheim") && !n.includes("karlich")) return "muelheim"; // не путать с Mülheim-Kärlich
  return null;
}

async function fetchJson(url, ms = 2500) {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), ms);
  try {
    const r = await fetch(url, { signal: ctrl.signal });
    return r.ok ? await r.json() : null;
  } finally {
    clearTimeout(timer);
  }
}

async function detectCity() {
  const forced = params.get("city");
  if (forced && CITIES[forced]) return forced;

  try {
    const saved = sessionStorage.getItem("city");
    if (saved !== null) return saved || null;
  } catch (e) {}

  let found = null;
  const sources = [
    ["https://get.geojs.io/v1/ip/geo.json", (d) => matchCity(d.city, d.country_code)],
    ["https://ipapi.co/json/",              (d) => matchCity(d.city, d.country_code)],
  ];
  for (const [url, pick] of sources) {
    try {
      const d = await fetchJson(url);
      if (d) { found = pick(d); break; }
    } catch (e) {} // сервис недоступен/заблокирован — пробуем следующий
  }

  try { sessionStorage.setItem("city", found || ""); } catch (e) {}
  return found;
}

/* =========================================================
   АНАЛИТИКА: клики по WhatsApp / звонку = конверсии.
   Подключится автоматически, когда добавишь Google Ads (gtag) или Meta Pixel (fbq).
   ========================================================= */
function track(channel, extra) {
  const data = { event: "lead_click", channel, city: city || "unknown", landing: LANDING || "home", ...extra };
  (window.dataLayer = window.dataLayer || []).push(data);
  if (typeof window.gtag === "function") window.gtag("event", "generate_lead", data);
  if (typeof window.fbq === "function") window.fbq("track", "Contact", data);
}

document.addEventListener("click", (e) => {
  const a = e.target.closest("a[href]");
  if (!a) return;
  if (a.href.startsWith("https://wa.me/")) track("whatsapp");
  else if (a.href.startsWith("tel:")) track("phone");
});

/* =========================================================
   ФИЛЬТР УСЛУГ
   ========================================================= */
function applyFilter(cat) {
  document.querySelectorAll("#servicesGrid .card").forEach((c) => {
    c.classList.toggle("is-hidden", cat !== "all" && c.dataset.cat !== cat);
  });
}

document.getElementById("serviceTabs").addEventListener("click", (e) => {
  const tab = e.target.closest(".tab");
  if (!tab) return;
  document.querySelectorAll(".tab").forEach((x) => x.classList.toggle("is-active", x === tab));
  applyFilter(tab.dataset.cat);
});

/* =========================================================
   МОБИЛЬНОЕ МЕНЮ
   ========================================================= */
const burger = document.getElementById("burger");
const nav = document.getElementById("nav");
burger.addEventListener("click", () => {
  const open = nav.classList.toggle("is-open");
  burger.setAttribute("aria-expanded", open);
});
nav.addEventListener("click", (e) => {
  if (e.target.tagName === "A") {
    nav.classList.remove("is-open");
    burger.setAttribute("aria-expanded", "false");
  }
});

/* =========================================================
   ФОРМА → WhatsApp
   ========================================================= */
document.getElementById("requestForm").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.target;
  const name = f.name.value.trim();
  f.name.classList.toggle("is-invalid", !name);
  if (!name) { f.name.focus(); return; }

  const sel = f.service;
  const lines = [
    `Hallo! Ich interessiere mich für: ${sel.options[sel.selectedIndex].text}`,
    `Name: ${name}`,
  ];
  if (f.zip.value.trim()) lines.push(`PLZ/Stadtteil: ${f.zip.value.trim()}`);
  if (f.msg.value.trim()) lines.push(`Aufgabe: ${f.msg.value.trim()}`);
  track("whatsapp_form", { service: sel.value });
  window.open(waLink(lines.join("\n")), "_blank", "noopener");
});

/* =========================================================
   СТАРТ
   ========================================================= */
document.getElementById("year").textContent = new Date().getFullYear();
renderContacts();
renderServices();
renderFaq();
renderFormOptions();
renderCity(); // сразу показываем «все 4 города», потом уточняем по IP
detectCity().then((c) => { city = c; renderCity(); });
