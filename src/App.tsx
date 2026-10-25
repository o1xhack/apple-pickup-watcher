import { invoke } from "@tauri-apps/api/core";
import { t } from "@/lib/i18n";
import { openUrl } from "@tauri-apps/plugin-opener";
import { productLabel } from "@/lib/display";
import { type ReactNode, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import {
  AlertTriangle,
  BellRing,
  ChevronDown,
  ChevronUp,
  Pause,
  Play,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";

import { Combobox } from "@/components/Combobox";
import { MultiCombobox } from "@/components/MultiCombobox";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Switch } from "@/components/ui/switch";
import {
  capacityOptions,
  colorOptions,
  familyOptions,
  keepAvailable,
  productsForSelection,
} from "@/lib/product-selection";
import {
  cleanPushRows,
  PUSH_KIND_LABEL,
  type PushKind,
  pushSummary,
  samePushUrls,
  savedPushAddresses,
} from "@/lib/push";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

import {
  changeLocale,
  clearNearbyStores,
  findNearbyStores,
  connect,
  refreshProducts,
  saveSettings,
  setCategory,
  setIntervalSeconds,
  setRepeatInStock,
  setLanguage,
  setPushUrls,
  setTargets,
  startWatching,
  stopWatching,
  testNotify,
  watcherStore,
} from "@/lib/store";
import {
  type Availability,
  type Category,
  describeAdvice,
  describeAvailability,
  formatTime,
  isUntrusted,
  type Product,
  type Settings,
  type StatusTone,
  type Target,
  targetKey,
} from "@/lib/types";
import { cn } from "@/lib/utils";

/** 四种展示状态各自的样式。 */
const TONE_CLASS: Record<StatusTone, string> = {
  inStock: "bg-in-stock/15 text-in-stock border-in-stock/30 font-medium",
  outOfStock: "bg-muted text-muted-foreground border-transparent",
  // 「未知」必须和「无货」长得完全不一样。这是整个项目的意义所在：上游把查询
  // 失败显示成「无货」，用户对着一个早已失效的程序空等了大半年。
  unknown: "bg-unknown/15 text-unknown border-unknown/40 font-medium",
  pending: "bg-transparent text-muted-foreground/60 border-dashed",
};

function StatusBadge({ availability }: { availability: Availability }) {
  const { label, tone, detail } = describeAvailability(availability);
  const badge = (
    <Badge variant="outline" className={`min-w-18 justify-center ${TONE_CLASS[tone]}`}>
      {label}
    </Badge>
  );
  if (!detail) return badge;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="cursor-help">{badge}</span>
      </TooltipTrigger>
      <TooltipContent className="max-w-90">{detail}</TooltipContent>
    </Tooltip>
  );
}

/**
 * 一块带标题的分区。「在哪儿加什么」和「怎么跑」分开放，控件不再挤成一排。
 *
 * 传 `onToggle` 就可以折叠：标题行整行可点，折叠后只剩标题和一行摘要。
 */
function Panel({
  title,
  hint,
  open = true,
  onToggle,
  children,
}: {
  title: string;
  hint?: string;
  open?: boolean;
  onToggle?: () => void;
  children: ReactNode;
}) {
  const heading = (
    <>
      <h2 className="shrink-0 text-sm font-medium">{title}</h2>
      {hint !== undefined && (
        <p className="text-muted-foreground min-w-0 flex-1 truncate text-xs" title={hint}>
          {hint}
        </p>
      )}
      {onToggle !== undefined &&
        (open ? (
          <ChevronUp className="text-muted-foreground size-4 shrink-0" />
        ) : (
          <ChevronDown className="text-muted-foreground size-4 shrink-0" />
        ))}
    </>
  );
  return (
    <section className="bg-card text-card-foreground rounded-xl border shadow-xs">
      {onToggle !== undefined ? (
        <button
          type="button"
          aria-expanded={open}
          onClick={onToggle}
          className={cn(
            "hover:bg-muted/40 flex w-full items-center gap-3 px-4 py-2 text-left",
            open ? "rounded-t-xl" : "rounded-xl",
          )}
        >
          {heading}
        </button>
      ) : (
        <div className="flex items-center gap-3 px-4 py-2">{heading}</div>
      )}
      {open && <div className="grid gap-2.5 border-t px-4 pt-3 pb-4">{children}</div>}
    </section>
  );
}

const SETTINGS_OPEN_KEY = "apw.settingsOpen";

/** 上次是展开还是收起。读不到（首次启动、无 localStorage）按展开。 */
function readSavedZip(): string {
  try { return localStorage.getItem("apw-us-pickup-zip") ?? ""; } catch { return ""; }
}

function readSettingsOpen(): boolean {
  try {
    return localStorage.getItem(SETTINGS_OPEN_KEY) !== "0";
  } catch {
    return true;
  }
}

/** 收起时的一行摘要：设置折起来了，但它们是什么一眼还在。 */
function settingsSummary(s: Settings): string {
  return [
    t(`每 ${s.intervalSeconds} 秒查一轮`),
    t(pushSummary(savedPushAddresses(s))),
    (s.proxies ?? []).length > 0 ? t(`代理 ${s.proxies.length} 条`) : t("无代理"),
    s.soundEnabled ? t("提示音开") : t("提示音关"),
    s.openBagOnHit ? t("有货时开购物袋") : t("有货时不开购物袋"),
    s.notificationLinkMode === "app" ? t("打开 Apple Store App") : s.paymentPreference === "apple_card_monthly_installments" ? t("网页 · Apple Card 分期") : t("网页 · 不预选付款"),
  ].join(" · ");
}

/** 运行状态。一个会呼吸的点比一行灰字更像「它还活着」。 */
function RunState({ running }: { running: boolean }) {
  return (
    <span className="text-muted-foreground flex items-center gap-2 text-sm">
      <span className="relative flex size-2">
        {running && (
          <span className="bg-in-stock/60 absolute inline-flex size-full animate-ping rounded-full [animation-duration:2.4s]" />
        )}
        <span
          className={cn(
            "relative inline-flex size-2 rounded-full",
            running ? "bg-in-stock" : "bg-muted-foreground/40",
          )}
        />
      </span>
      {running ? t("监听中") : t("已暂停")}
    </span>
  );
}

/** 一个统计数字。数字等宽：每轮查询都会变，几个并排时不该来回抖。 */
function Stat({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone?: "inStock" | "unknown";
}) {
  return (
    <span className="bg-card inline-flex items-baseline gap-1.5 rounded-md border px-2.5 py-1 text-sm shadow-xs">
      <span className="text-muted-foreground text-xs">{label}</span>
      <span
        className={cn(
          "font-semibold tabular-nums",
          value > 0 && tone === "inStock" && "text-in-stock",
          value > 0 && tone === "unknown" && "text-unknown",
        )}
      >
        {value}
      </span>
    </span>
  );
}

/** 日志行。时间戳弱化，到货那几行用有货色标出来，一屏灰字里一眼能找到。 */
function LogLine({ line }: { line: string }) {
  const match = /^\[(\d{2}:\d{2}:\d{2})\] ?(.*)$/s.exec(line);
  if (!match) return <div className="whitespace-pre-wrap">{t(line)}</div>;
  const [, stamp, text = ""] = match;
  return (
    <div className="whitespace-pre-wrap">
      <span className="text-muted-foreground/60">{stamp}</span>{" "}
      <span className={text.startsWith("有货") ? "text-in-stock font-medium" : "text-muted-foreground"}>
        {t(text)}
      </span>
    </div>
  );
}

interface WebAccessInfo {
  localUrl: string;
  lanEnabled: boolean;
  lanUrls: { label: string; url: string }[];
}

export default function App() {
  const [webAccess, setWebAccess] = useState<WebAccessInfo | null>(null);
  const [webNotice, setWebNotice] = useState("");
  const ui = useSyncExternalStore(watcherStore.subscribe, watcherStore.getSnapshot);
  useEffect(() => {
    void invoke<WebAccessInfo>("web_access").then(setWebAccess).catch(error => setWebNotice(String(error)));
  }, [ui.settings.lanEnabled]);
  async function copyWebLink(url: string) {
    try { await navigator.clipboard.writeText(url); setWebNotice("链接已复制"); }
    catch (error) { setWebNotice(String(error)); }
  }

  useEffect(() => {
    void connect();
    // 刻意不在清理函数里断开：这是应用级的单一连接，窗口活着它就该活着。
    // StrictMode 的重复调用由 connect 内部去重。
  }, []);

  // 门店可以一次选好几家：同城门店合并成一次请求之后，多选不增加请求量。
  const [storeNumbers, setStoreNumbers] = useState<string[]>([]);
  const [pickupZipDraft, setPickupZipDraft] = useState<string | null>(null);
  const pickupZip = pickupZipDraft ?? (readSavedZip() || (ui.settings.targets ?? []).find(
    (t) => t.locale === "en_US" && t.pickupLocation,
  )?.pickupLocation || "");
  const validPickupZip = !pickupZip.trim() || /^\d{5}(-\d{4})?$/.test(pickupZip.trim());
  const usingZip = ui.settings.locale === "en_US" && !!pickupZip.trim();
  const nearbyReady = ui.nearbyStores?.zip === pickupZip.trim();
  useEffect(() => {
    setStoreNumbers([]);
    clearNearbyStores();
    if (!usingZip || !validPickupZip) return;
    const zip = pickupZip.trim();
    try { localStorage.setItem("apw-us-pickup-zip", zip); } catch { /* 存储不可用仍可查询。 */ }
    let active = true;
    const timer = setTimeout(() => {
      void findNearbyStores(zip).then((stores) => {
        if (active && stores) setStoreNumbers(stores.filter((s) => s.distance !== null).slice(0, 3).map((s) => s.number));
      });
    }, 600);
    return () => { active = false; clearTimeout(timer); };
  }, [ui.settings.locale, pickupZip]);
  // 非 iPhone 品类直接选一个完整型号。
  const [partNumber, setPartNumber] = useState("");
  // iPhone 按机型 → 容量 → 颜色三级选择，三级都可多选：发售当晚常见的是
  // 「这两款、这三个颜色，哪个有货要哪个」，一台一台加太慢。三级组合里目录中
  // 存在的型号全部加入。
  const [families, setFamilies] = useState<string[]>([]);
  const [capacities, setCapacities] = useState<string[]>([]);
  const [colors, setColors] = useState<string[]>([]);
  const [criticalAlert, setCriticalAlert] = useState(false);
  // 推送地址：Bark 和飞书是同一个列表，一行一个。null 表示还没动过，跟着后端走；
  // 动过之后以这份为准，保持用户填写的顺序，渠道标签按后端的归类显示。
  const [pushDraft, setPushDraft] = useState<string[] | null>(null);
  // 代理地址和推送地址一样一行一个，草稿的规矩也相同，见 pushDraft。
  const [proxyDraft, setProxyDraft] = useState<string[] | null>(null);
  const [intervalDraft, setIntervalDraft] = useState<number | null>(null);
  // 设置改一次就放着不动，折起来把纵向空间还给表格；记住上次的选择。
  const [settingsOpen, setSettingsOpen] = useState(readSettingsOpen());
  function toggleSettings() {
    const next = !settingsOpen;
    setSettingsOpen(next);
    try {
      localStorage.setItem(SETTINGS_OPEN_KEY, next ? "1" : "0");
    } catch {
      // 存不下就每次都展开，不值得报错。
    }
  }

  const savedPush = useMemo(() => savedPushAddresses(ui.settings), [ui.settings]);
  const savedPushKind = new Map<string, PushKind>(savedPush.map((p) => [p.url, p.kind]));
  const pushRows = pushDraft ?? savedPush.map((p) => p.url);
  // 一个地址都没有时留一个空行，直接粘贴就行，不用先点「添加」。
  const visiblePushRows = pushRows.length > 0 ? pushRows : [""];
  const lastPushRow = visiblePushRows[visiblePushRows.length - 1] ?? "";

  function editPushRow(index: number, value: string) {
    setPushDraft(visiblePushRows.map((row, i) => (i === index ? value : row)));
  }

  // 失焦和删除都走这里：整理后整组交给后端归类保存；没有实际改动就不写盘。
  // 整理后的列表留作草稿：保存是异步的，这时退回「跟着后端走」，界面会先闪回
  // 旧地址，紧接着点「添加」还会以旧列表为底，把刚填的地址弄丢。
  function commitPushRows(rows: string[]) {
    const urls = cleanPushRows(rows);
    setPushDraft(urls);
    if (!samePushUrls(urls, savedPush.map((p) => p.url))) {
      void setPushUrls(urls);
    }
  }
  // 旧设置文件没有 proxies 字段，读上来是 undefined，当作空列表。
  const savedProxies = ui.settings.proxies ?? [];
  const proxyRows = proxyDraft ?? savedProxies;
  const visibleProxyRows = proxyRows.length > 0 ? proxyRows : [""];
  const lastProxyRow = visibleProxyRows[visibleProxyRows.length - 1] ?? "";

  function editProxyRow(index: number, value: string) {
    setProxyDraft(visibleProxyRows.map((row, i) => (i === index ? value : row)));
  }

  // 每个代理是一条额外的出口线路：Apple 的配额按出口 IP 计，多一条线路多一份配额，
  // 被拦时自动换下一条。后端只留能解析的地址，留不下的那行会标「未生效」。
  function commitProxyRows(rows: string[]) {
    const proxies = cleanPushRows(rows);
    setProxyDraft(proxies);
    if (!samePushUrls(proxies, savedProxies)) {
      void saveSettings({ ...ui.settings, proxies });
    }
  }
  const intervalValue = intervalDraft ?? ui.settings.intervalSeconds;

  const selectableStores = usingZip ? (nearbyReady ? ui.nearbyStores?.stores ?? [] : []) : ui.stores;
  const storeOptions = usingZip
    ? (nearbyReady ? ui.nearbyStores?.stores ?? [] : []).map((s) => ({
      value: s.number,
      label: `${s.title} · ${s.distanceLabel ?? (s.distance !== null ? `${s.distance.toFixed(2)} mi` : t("距离未知"))}`,
    }))
    : ui.stores.map((s) => ({ value: s.number, label: s.title }));
  // 只列当前品类。四个品类的型号加起来好几百条，全堆进一个下拉框，
  // 想找一台 Mac 得先划过所有 iPhone。
  const productOptions = useMemo(
    () =>
      ui.products
        .filter((p) => p.category === ui.category)
        .map((p) => ({ value: p.partNumber, label: productLabel(p.title) })),
    [ui.products, ui.category, ui.settings.language],
  );
  const iphoneFamilyOptions = useMemo(
    () => familyOptions(ui.products, ui.category),
    [ui.products, ui.category, ui.settings.language],
  );
  const iphoneCapacityOptions = useMemo(
    () => capacityOptions(ui.products, ui.category, families),
    [ui.products, ui.category, families],
  );
  const iphoneColorOptions = useMemo(
    () => colorOptions(ui.products, ui.category, families, capacities),
    [ui.products, ui.category, families, capacities, ui.settings.language],
  );
  // 点「添加」时会加入的型号。iPhone 是三级选择的组合，其他品类就是选中的那一个。
  const selectedProducts = useMemo<Product[]>(() => {
    if (ui.category === "iphone") {
      return productsForSelection(ui.products, ui.category, families, capacities, colors);
    }
    const product = ui.products.find((p) => p.partNumber === partNumber);
    return product ? [product] : [];
  }, [ui.products, ui.category, families, capacities, colors, partNumber]);

  function resetProductSelection() {
    setPartNumber("");
    setFamilies([]);
    setCapacities([]);
    setColors([]);
  }

  // 上级变了，下级只去掉不再可选的项，仍然成立的留着（理由见 keepAvailable）。
  function onFamiliesChange(next: string[]) {
    setFamilies(next);
    const nextCapacities = keepAvailable(
      capacities,
      capacityOptions(ui.products, ui.category, next),
    );
    setCapacities(nextCapacities);
    setColors(
      keepAvailable(colors, colorOptions(ui.products, ui.category, next, nextCapacities)),
    );
  }

  function onCapacitiesChange(next: string[]) {
    setCapacities(next);
    setColors(keepAvailable(colors, colorOptions(ui.products, ui.category, families, next)));
  }

  const targets = useMemo(() => ui.rows.map((r) => r.target), [ui.rows]);

  const summary = useMemo(() => {
    let inStock = 0;
    let outOfStock = 0;
    let untrusted = 0;
    let notOpen = 0;
    for (const r of ui.rows) {
      if (r.availability.kind === "in_stock") inStock += 1;
      else if (r.availability.kind === "out_of_stock") outOfStock += 1;
      if (isUntrusted(r.availability)) untrusted += 1;
      if (r.availability.kind === "unknown" && r.availability.reason === "pickup_not_open") notOpen += 1;
    }
    return { inStock, outOfStock, untrusted, notOpen };
  }, [ui.rows]);

  const canAdd = storeNumbers.length > 0 && selectedProducts.length > 0
    && (ui.settings.locale !== "en_US" || validPickupZip)
    && (!usingZip || (nearbyReady && !ui.nearbyLoading));

  async function onAdd() {
    if (!canAdd) return;

    // 门店 × 型号，每个组合一条目标；已经在列表里的跳过，不重复。
    // 按门店分组排，表格里同一家店的几台机器挨在一起。
    const existing = new Set(targets.map(targetKey));
    const added: Target[] = [];
    for (const number of storeNumbers) {
      const store = selectableStores.find((s) => s.number === number);
      if (!store) continue;
      for (const product of selectedProducts) {
        const next: Target = {
          locale: ui.settings.locale,
          storeNumber: store.number,
          storeTitle: store.title,
          partNumber: product.partNumber,
          productName: product.title,
          ...(criticalAlert ? { criticalAlert: true } : {}),
          ...(ui.settings.locale === "en_US" && pickupZip.trim()
            ? { pickupLocation: pickupZip.trim() } : {}),
          ...(product.companionPart ? { companionPart: product.companionPart } : {}),
        };
        const key = targetKey(next);
        if (existing.has(key)) continue;
        existing.add(key);
        added.push(next);
      }
    }
    if (added.length === 0) return;
    if (!await setTargets([...targets, ...added], targets)) return;
    // 只清型号，门店留着：接着给同一批门店加下一个型号是最常见的操作。
    resetProductSelection();
  }

  async function onRemove(t: Target) {
    await setTargets(targets.filter((x) => targetKey(x) !== targetKey(t)), targets);
  }

  return (
    <TooltipProvider delayDuration={200}>
      <div className="mx-auto flex h-screen max-w-6xl flex-col gap-3 p-5">
        <header className="flex items-start justify-between gap-4">
          <div>
            <h1 className="text-xl font-semibold tracking-tight">{t("Apple 取货监控")}</h1>
            <p className="text-muted-foreground text-sm">{t("盯 Apple 直营店的到店取货库存，有货立刻提醒")}</p>
          </div>
          <div className="flex items-center gap-4">
            <Button variant="outline" onClick={() => void openUrl("http://127.0.0.1:17842")}>{t("网页 / Status")}</Button>
            <label className="text-sm flex items-center gap-2">{t("语言")}
              <select aria-label={t("语言")} className="rounded-md border bg-background p-2" value={ui.settings.language ?? "zh"} onChange={(e) => void setLanguage(e.target.value as "zh" | "en").catch(error => window.alert(String(error)))}>
                <option value="zh">中文</option><option value="en">English</option>
              </select>
            </label>
            <RunState running={ui.running} />
            {ui.running ? (
              <Button variant="secondary" onClick={() => void stopWatching()}>
                <Pause />{t("暂停")}</Button>
            ) : (
              <Button onClick={() => void startWatching()} disabled={ui.rows.length === 0}>
                <Play />{t("开始")}</Button>
            )}
          </div>
        </header>
        <details className="text-muted-foreground text-xs"><summary className="cursor-pointer">{t("关于 · 网页端 · 开源许可")}</summary><p className="py-2">{t("Apple 取货监控 / Apple Pickup Watcher · 网页：http://127.0.0.1:17842 · 基于 ENCHIGO/apple-pickup-watcher 与 Hteen/apple-store-helper，GPL-3.0-or-later。自由软件，无担保。源码与许可：https://github.com/o1xhack/apple-pickup-watcher")}</p></details>

        <details className="rounded-lg border p-3 text-sm" onToggle={event => { if(event.currentTarget.open) void invoke<WebAccessInfo>("web_access").then(setWebAccess).catch(error => setWebNotice(String(error))); }}>
          <summary className="cursor-pointer font-medium">{t("网页访问")}</summary>
          <div className="mt-3 flex flex-wrap items-center gap-2"><span>{t("本机地址")}</span><code>http://127.0.0.1:17842</code><Button size="sm" variant="outline" onClick={() => void openUrl("http://127.0.0.1:17842")}>{t("打开")}</Button><Button size="sm" variant="outline" onClick={() => void copyWebLink("http://127.0.0.1:17842")}>{t("复制链接")}</Button></div>
          <div className="mt-3 flex items-center gap-2"><Switch id="lan-access" checked={webAccess?.lanEnabled ?? false} onCheckedChange={(enabled) => void invoke<WebAccessInfo>("set_lan_enabled", {enabled}).then(setWebAccess).catch(error => setWebNotice(String(error)))} /><Label htmlFor="lan-access">{t("允许局域网访问")}</Label></div>
          <p className="my-2 text-muted-foreground text-xs">{t("开启后复制访问链接，同一 Wi-Fi 或有线网络中的设备即可查看和编辑。链接包含访问凭证，请仅分享给可信的人。")}</p>
          {webAccess?.lanEnabled && webAccess.lanUrls.length===0 && <p>{t("未检测到局域网地址，请检查 Wi-Fi 或网线连接。")}</p>}
          {webAccess?.lanUrls.map(link => <div key={link.label} className="mt-2 flex flex-wrap items-center gap-2"><code>{link.label}</code><Button size="sm" variant="outline" onClick={() => void openUrl(link.url)}>{t("打开")}</Button><Button size="sm" variant="outline" onClick={() => void copyWebLink(link.url)}>{t("复制链接")}</Button></div>)}
          {webNotice && <p className="mt-2 text-xs">{t(webNotice)}</p>}
        </details>

        {ui.trouble !== null && (
          // 只有标题是红的。原因和建议各占一行、用正常的字色：三段红字堆在一起，
          // 用户反而分不清哪句是原因、哪句是该做的事。
          <div
            role="alert"
            className="border-destructive/30 bg-destructive/5 border-l-destructive grid gap-1.5 rounded-lg border border-l-4 px-4 py-3 text-sm"
          >
            <p className="text-destructive flex flex-wrap items-center gap-x-2 font-medium">
              <AlertTriangle className="size-4" />{t("监控当前不可信")}<span className="text-muted-foreground font-normal">{t("恢复之前，表格里的状态不代表门店的真实库存")}</span>
            </p>
            <p className="pl-6">{ui.trouble.reason}</p>
            {ui.trouble.advice !== null && (
              // 用户自己能做的那件事单独拎出来，而且要短：issue #3 里那位对着反复
              // 告警的窗口干等了三个小时，缺的不是解释，是一句「换条网络」。
              <p className="text-muted-foreground pl-6">
                <span className="text-foreground font-medium">{t("建议")}</span>{" "}
                {describeAdvice(ui.trouble.advice)}
              </p>
            )}
          </div>
        )}

        <Panel
          title={t("添加监控目标")}
          hint={t("不同品类、不同门店可以混着加；门店和 iPhone 的机型、容量、颜色都可多选，组合一次全部加入，添加后自动开始监控")}
        >
          <div className="flex flex-wrap items-end gap-3">
            <div className="grid gap-1.5">
              <Label>{t("地区")}</Label>
              <Combobox
                className="w-36"
                options={ui.regions.map((r) => ({ value: r.locale, label: t(r.title) }))}
                value={ui.settings.locale}
                onChange={(locale) => {
                  // 换地区后旧的门店和型号都不再适用，清掉待添加的选择。
                  setStoreNumbers([]);
                  resetProductSelection();
                  void changeLocale(locale);
                }}
                placeholder={t("选择地区")}
                searchPlaceholder={t("搜索地区…")}
                emptyText={t("没有匹配的地区")}
              />
            </div>

            <div className="grid gap-1.5">
              <Label>{t("品类")}</Label>
              <Combobox
                className="w-36"
                options={ui.categories.map((c) => ({ value: c.value, label: t(c.title) }))}
                value={ui.category}
                onChange={(value) => {
                  // 换品类后旧的型号不再在下拉框里，清掉待添加的选择。门店不用清，
                  // 它跟品类无关。
                  resetProductSelection();
                  setCategory(value as Category);
                }}
                placeholder={t("选择品类")}
                searchPlaceholder={t("搜索品类…")}
                emptyText={t("没有匹配的品类")}
                disabled={ui.categories.length === 0}
              />
            </div>

            {ui.settings.locale === "en_US" && (
              <div className="grid gap-1.5">
                <Label htmlFor="pickup-zip">{t("查询 ZIP（可选）")}</Label>
                <Input
                  id="pickup-zip"
                  className="w-36"
                  value={pickupZip}
                  onChange={(event) => {
                    setPickupZipDraft(event.target.value);
                    if (!event.target.value.trim()) {
                      try { localStorage.removeItem("apw-us-pickup-zip"); } catch { /* 可继续使用。 */ }
                    }
                  }}
                  placeholder={t("5 位或 ZIP+4")}
                  maxLength={10}
                  aria-invalid={!validPickupZip}
                  aria-describedby="pickup-zip-help"
                />
              </div>
            )}

            <div className="grid gap-1.5">
              <Label>{t("门店（可多选）")}</Label>
              <MultiCombobox
                className="w-64"
                options={storeOptions}
                values={storeNumbers}
                onChange={setStoreNumbers}
                placeholder={ui.nearbyLoading ? t("正在查找附近门店…") : t("选择自提门店")}
                searchPlaceholder={t("搜索门店…")}
                emptyText={t("没有匹配的门店")}
                countLabel={(n) => t(`已选 ${n} 家门店`)}
                disabled={storeOptions.length === 0 || ui.nearbyLoading}
              />
            </div>
          </div>

          {ui.settings.locale === "en_US" && (
            <p id="pickup-zip-help" className={cn("text-xs", validPickupZip ? "text-muted-foreground" : "text-destructive")}>
              {validPickupZip
                ? (usingZip
                  ? (ui.nearbyLoading ? t("正在按 ZIP 查找附近门店…")
                    : nearbyReady ? t(`已按距 ${pickupZip.trim()} 的距离由近到远排列 ${selectableStores.length} 家门店；默认勾选最近三家，可自行调整，再点击「添加」。距离为 Apple 按邮编估算。`)
                      : t("输入完整 ZIP 后会自动查找附近门店，按距离从近到远排列。"))
                  : t("输入 ZIP 自动查找并按距离排列附近门店；留空显示全部美国门店。添加后保存监控目标。"))
                : t("请输入有效的 5 位 ZIP 或 ZIP+4，例如 95014 或 95014-1234。")}
            </p>
          )}
          {usingZip && ui.nearbyError && (
            <div role="alert" className="text-destructive text-sm">{t("查找附近门店失败：")}{ui.nearbyError}
              <Button variant="ghost" size="sm" onClick={() => {
                void findNearbyStores(pickupZip.trim()).then((stores) => {
                  if (stores) setStoreNumbers(stores.filter((s) => s.distance !== null).slice(0, 3).map((s) => s.number));
                });
              }}>{t("重试")}</Button>
            </div>
          )}

          <div className="flex flex-wrap items-end gap-3">
            {ui.category === "iphone" ? (
              <>
                <div className="grid gap-1.5">
                  <Label>{t("机型（可多选）")}</Label>
                  <MultiCombobox
                    className="w-48"
                    options={iphoneFamilyOptions}
                    values={families}
                    onChange={onFamiliesChange}
                    placeholder={t("选择机型")}
                    searchPlaceholder={t("搜索机型…")}
                    emptyText={t("没有匹配的机型")}
                    countLabel={(n) => t(`已选 ${n} 款机型`)}
                    disabled={iphoneFamilyOptions.length === 0}
                  />
                </div>

                <div className="grid gap-1.5">
                  <Label>{t("容量（可多选）")}</Label>
                  <MultiCombobox
                    className="w-36"
                    options={iphoneCapacityOptions}
                    values={capacities}
                    onChange={onCapacitiesChange}
                    placeholder={t("选择容量")}
                    searchPlaceholder={t("搜索容量…")}
                    emptyText={t("没有匹配的容量")}
                    countLabel={(n) => t(`已选 ${n} 种容量`)}
                    disabled={families.length === 0 || iphoneCapacityOptions.length === 0}
                  />
                </div>

                <div className="grid gap-1.5">
                  <Label>{t("颜色（可多选）")}</Label>
                  <MultiCombobox
                    className="w-40"
                    options={iphoneColorOptions}
                    values={colors}
                    onChange={setColors}
                    placeholder={t("选择颜色")}
                    searchPlaceholder={t("搜索颜色…")}
                    emptyText={t("没有匹配的颜色")}
                    countLabel={(n) => t(`已选 ${n} 种颜色`)}
                    disabled={capacities.length === 0 || iphoneColorOptions.length === 0}
                  />
                </div>
              </>
            ) : (
              <div className="grid gap-1.5">
                <Label>{t("型号")}</Label>
                <Combobox
                  className="w-96"
                  options={productOptions}
                  value={partNumber}
                  onChange={setPartNumber}
                  placeholder={t("选择型号")}
                  searchPlaceholder={t("搜索型号…")}
                  emptyText={t("没有匹配的型号")}
                  disabled={productOptions.length === 0}
                />
              </div>
            )}

            <div className="flex items-center gap-2 pb-2">
              <Switch id="target-critical" checked={criticalAlert} onCheckedChange={setCriticalAlert} />
              <Label htmlFor="target-critical">{t("Critical Alerts（重要警告）")}</Label>
            </div>
            <span className="text-muted-foreground max-w-72 pb-2 text-xs">{t("仅应用于本次添加的目标。请在 iPhone 设置中允许 Bark 的「重要警告」。")}</span>
            <Button variant="secondary" onClick={() => void onAdd()} disabled={!canAdd}>
              <Plus />{t("添加")}</Button>
            {selectedProducts.length > 1 && (
              // 多选时把要加的规模点明：三级各勾几项之后，组合数并不直观。
              <span
                className="text-muted-foreground pb-2 text-sm"
                title={selectedProducts.map((p) => productLabel(p.title)).join("、")}
              >
                {selectedProducts.length}{" " + t("个型号 ×") + " "}{storeNumbers.length}{" " + t("家门店")}</span>
            )}

            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={t("从 Apple 官网更新当前品类的型号列表")}
                  disabled={ui.refreshing}
                  onClick={() => void refreshProducts()}
                >
                  <RefreshCw className={ui.refreshing ? "animate-spin" : undefined} />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{t("从 Apple 官网更新当前品类的型号列表。新机发布后用这个，不必等程序更新。")}</TooltipContent>
            </Tooltip>
          </div>
        </Panel>

        <Panel
          title={t("运行设置")}
          hint={settingsOpen ? t("改动即时保存") : settingsSummary(ui.settings)}
          open={settingsOpen}
          onToggle={toggleSettings}
        >
          {/* 两栏：左边是查询与本机提醒，右边是推送地址。推送地址一行一个会占好几行，
              和别的设置叠成一列的话，默认窗口高度下监控列表会被挤得看不见。 */}
          <div className="flex flex-wrap items-start gap-x-8 gap-y-4">
            <div className="grid min-w-80 grow-[2] basis-80 gap-3">
              <div className="grid gap-1.5">
                {/* 标题行和右栏「推送地址」一样高，两栏的输入框才能对齐。 */}
                <div className="flex h-3.5 items-center gap-x-2">
                  <Label htmlFor="proxy-0" className="shrink-0 whitespace-nowrap">{t("代理地址（可选）")}</Label>
                  <span className="text-muted-foreground min-w-0 truncate text-xs leading-none">{t("http / https / socks5，一行一个")}</span>
                  {lastProxyRow.trim() !== "" && (
                    <Button
                      variant="link"
                      size="sm"
                      className="text-muted-foreground hover:text-foreground ml-auto h-3.5 shrink-0 gap-1 p-0 text-xs leading-none has-[>svg]:px-0 [&_svg]:size-3"
                      onClick={() => setProxyDraft([...cleanPushRows(visibleProxyRows), ""])}
                    >
                      <Plus />{t("添加代理")}</Button>
                  )}
                </div>
                {visibleProxyRows.map((row, index) => {
                  // 保存后后端没留下这一行：地址解析不了，这条线路不存在。
                  const rejected =
                    proxyDraft !== null && row.trim() !== "" && !savedProxies.includes(row.trim());
                  return (
                    <div key={index} className="flex items-center gap-2">
                      <Input
                        id={`proxy-${index}`}
                        aria-label={t(`代理地址 ${index + 1}`)}
                        aria-invalid={rejected || undefined}
                        className="select-text"
                        placeholder={t("socks5://127.0.0.1:1080 或 http://user:pass@host:port")}
                        value={row}
                        autoFocus={proxyDraft !== null && row === "" && index > 0}
                        onChange={(e) => editProxyRow(index, e.target.value)}
                        onBlur={() => commitProxyRows(visibleProxyRows)}
                      />
                      {rejected && (
                        <Badge
                          variant="outline"
                          className="text-destructive border-destructive/40 shrink-0"
                          title={t("地址格式不对，这条线路没有生效。支持 http、https、socks5、socks5h，例如 socks5://127.0.0.1:1080")}
                        >{t("未生效")}</Badge>
                      )}
                      <Button
                        variant="ghost"
                        size="icon"
                        aria-label={t(`删除代理地址 ${index + 1}`)}
                        disabled={visibleProxyRows.length === 1 && row === ""}
                        onClick={() =>
                          commitProxyRows(visibleProxyRows.filter((_, i) => i !== index))
                        }
                      >
                        <Trash2 />
                      </Button>
                    </div>
                  );
                })}
              </div>

              <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
                <div className="flex items-center gap-2">
                  <Label htmlFor="interval" className="whitespace-nowrap">{t("查询间隔")}</Label>
                  <Input
                    id="interval"
                    type="number"
                    min={5}
                    className="h-8 w-20 select-text"
                    value={intervalValue}
                    onChange={(e) => setIntervalDraft(e.target.valueAsNumber)}
                    onBlur={() => {
                      const s = Number.isFinite(intervalValue) ? Math.round(intervalValue) : 30;
                      setIntervalDraft(null);
                      void setIntervalSeconds(s);
                    }}
                  />
                  <span className="text-muted-foreground text-sm">{t("秒")}</span>
                </div>
                <div className="flex items-center gap-2">
                  <Switch
                    id="sound"
                    checked={ui.settings.soundEnabled}
                    onCheckedChange={(v) =>
                      void saveSettings({ ...ui.settings, soundEnabled: v })
                    }
                  />
                  <Label htmlFor="sound">{t("提示音")}</Label>
                </div>
                <div className="flex items-center gap-2">
                  <Switch
                    id="openbag"
                    checked={ui.settings.openBagOnHit}
                    onCheckedChange={(v) =>
                      void saveSettings({ ...ui.settings, openBagOnHit: v })
                    }
                  />
                  <Label htmlFor="openbag">{t("有货时打开购物袋")}</Label>
                </div>
              </div>
            </div>

            <div className="grid min-w-96 grow-[3] basis-96 gap-1.5">
              {/* 标题行和左栏的标签一样高，两栏的第一行输入框才能对齐。 */}
              <div className="flex h-3.5 items-center gap-x-2">
                <Label htmlFor="push-0" className="shrink-0 whitespace-nowrap">{t("推送地址（可选）")}</Label>
                <span className="text-muted-foreground min-w-0 truncate text-xs leading-none">{t("Bark 分组：Apple 库存监控；也支持飞书")}</span>
                {lastPushRow.trim() !== "" && (
                  <Button
                    variant="link"
                    size="sm"
                    className="text-muted-foreground hover:text-foreground ml-auto h-3.5 shrink-0 gap-1 p-0 text-xs leading-none has-[>svg]:px-0 [&_svg]:size-3"
                    onClick={() => setPushDraft([...cleanPushRows(visiblePushRows), ""])}
                  >
                    <Plus />{t("添加推送地址")}</Button>
                )}
                {/* 测试的就是这些推送（连同系统通知和提示音），放在它们的标题旁边。 */}
                <Button
                  variant="link"
                  size="sm"
                  className={`text-muted-foreground hover:text-foreground h-3.5 shrink-0 gap-1 p-0 text-xs leading-none has-[>svg]:px-0 [&_svg]:size-3 ${lastPushRow.trim() !== "" ? "" : "ml-auto"}`}
                  onClick={() => void testNotify()}
                >
                  <BellRing />{t("测试提醒")}</Button>
                <Button variant="link" size="sm" className="h-3.5 shrink-0 p-0 text-xs"
                  onClick={() => void testNotify(true)}>{t("测试重要警告")}</Button>
              </div>
              {visiblePushRows.map((row, index) => {
                const kind = savedPushKind.get(row.trim());
                return (
                  <div key={index} className="flex items-center gap-2">
                    {/* 渠道标签占固定宽度：还没保存的行留空，输入框照样对齐。 */}
                    <span className="flex w-12 shrink-0 justify-center">
                      {kind !== undefined && (
                        <Badge variant="outline" className="text-muted-foreground">
                          {PUSH_KIND_LABEL[kind]}
                        </Badge>
                      )}
                    </span>
                    <Input
                      id={`push-${index}`}
                      aria-label={t(`推送地址 ${index + 1}`)}
                      className="select-text"
                      placeholder={t("粘贴 Bark 地址，或飞书群机器人的 webhook 地址")}
                      value={row}
                      // 只有点「添加」新开的那一行自动聚焦；启动时的空行不抢焦点。
                      autoFocus={pushDraft !== null && row === "" && index > 0}
                      onChange={(e) => editPushRow(index, e.target.value)}
                      onBlur={() => commitPushRows(visiblePushRows)}
                    />
                    <Button
                      variant="ghost"
                      size="icon"
                      aria-label={t(`删除推送地址 ${index + 1}`)}
                      disabled={visiblePushRows.length === 1 && row === ""}
                      onClick={() =>
                        commitPushRows(visiblePushRows.filter((_, i) => i !== index))
                      }
                    >
                      <Trash2 />
                    </Button>
                  </div>
                );
              })}
              {savedPush.some((p) => p.kind === "feishu") && (
                // 飞书机器人最常见的配置错误是安全设置，只在真填了飞书地址时提醒。
                <p className="text-muted-foreground pl-14 text-xs">{t("飞书机器人的安全设置请选「自定义关键词」，填「有货」。")}</p>
              )}
            </div>
          </div>
          <div className="mt-4 grid gap-3 border-t pt-4">
            <div className="flex flex-wrap gap-4">
              <div className="grid gap-1.5">
                <Label htmlFor="notification-link-mode">{t("通知打开方式")}</Label>
                <select id="notification-link-mode" className="border-input bg-card h-9 rounded-md border px-3 text-sm"
                  value={ui.settings.notificationLinkMode ?? "web"}
                  onChange={(e) => void saveSettings({ ...ui.settings, notificationLinkMode: e.target.value as Settings["notificationLinkMode"] })}>
                  <option value="app">Apple Store App</option>
                  <option value="web">{t("网页（Apple 官网）")}</option>
                </select>
              </div>
              {ui.settings.notificationLinkMode !== "app" && (
                <div className="grid gap-1.5">
                  <Label htmlFor="payment-preference">{t("网页付款方式")}</Label>
                  <select id="payment-preference" className="border-input bg-card h-9 rounded-md border px-3 text-sm"
                    value={ui.settings.paymentPreference ?? "none"}
                    onChange={(e) => void saveSettings({ ...ui.settings, paymentPreference: e.target.value as Settings["paymentPreference"] })}>
                    <option value="none">{t("不预选付款方式")}</option>
                    <option value="apple_card_monthly_installments">{t("Apple Card Monthly Installments（分期）")}</option>
                  </select>
                </div>
              )}
            </div>
            <p className="text-muted-foreground text-xs">{t("设置应用于之后的到货通知。网页链接仍可能被 iOS 转到 Apple Store App，可复制链接到 Safari 地址栏打开。")}</p>
            {ui.settings.notificationLinkMode !== "app" && ui.settings.paymentPreference === "apple_card_monthly_installments" && (
              <p className="text-muted-foreground text-xs">{t("仅预选美国 iPhone 的 Apple Card 分期，不包含换购和运营商。手动完成换购后，需刷新原标签页恢复分期选择。")}</p>
            )}
          </div>
        </Panel>

        <div className="flex flex-wrap items-center gap-2">
          <Stat label={t("监控")} value={ui.rows.length} />
          <Button variant="outline" size="sm" disabled={targets.length === 0}
            onClick={() => void setTargets([], targets)}><Trash2 />{t("清空全部")}</Button>
          <Stat label={t("有货")} value={summary.inStock} tone="inStock" />
          <Stat label={t("无货")} value={summary.outOfStock} />
          <div className="ml-auto flex items-center gap-2" title={t("应用于所有监控：每轮确认有货都提醒，关闭后恢复连续有货去重。")}>
            <Switch id="repeat-in-stock" checked={ui.settings.repeatInStock}
              onCheckedChange={(enabled) => void setRepeatInStock(enabled)} />
            <Label htmlFor="repeat-in-stock">{t("有货重复提醒（全部监控）")}</Label>
          </div>
          {summary.notOpen > 0 && <Stat label={t("未开放")} value={summary.notOpen} />}
          {summary.untrusted > 0 && (
            // 「其中多少项查不到」单独点出来：这个数字大于 0 时，
            // 表格里那些「无货」也未必反映真实情况。
            <Stat label={t("查不到")} value={summary.untrusted} tone="unknown" />
          )}
          {ui.running && ui.pacing !== null && ui.pacing.paced && (
            // 请求预算在拉长间隔时明说，否则用户会以为设的 30 秒没生效。
            <span className="text-muted-foreground ml-auto text-sm">{t("冷却／恢复调度中，下一轮")}{ui.pacing.nextCheckInSecs}{t("秒后")}</span>
          )}
        </div>

        <section className="bg-card min-h-0 flex-1 overflow-hidden rounded-xl border shadow-xs">
          <ScrollArea className="h-full">
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead className="text-muted-foreground w-24 pl-4 text-xs">{t("状态")}</TableHead>
                  <TableHead className="text-muted-foreground text-xs">{t("门店")}</TableHead>
                  <TableHead className="text-muted-foreground text-xs">{t("型号")}</TableHead>
                  <TableHead className="text-muted-foreground text-xs">{t("重要警告")}</TableHead>
                  <TableHead className="text-muted-foreground w-24 text-xs">{t("最后检查")}</TableHead>
                  <TableHead className="w-12" />
                </TableRow>
              </TableHeader>
              <TableBody>
                {ui.rows.length === 0 ? (
                  <TableRow className="hover:bg-transparent">
                    <TableCell colSpan={6} className="text-muted-foreground h-28 text-center">
                      {ui.ready ? t("还没有监控目标。在上面选好门店和型号，点「添加」。") : t("正在载入…")}
                    </TableCell>
                  </TableRow>
                ) : (
                  ui.rows.map((row) => (
                    <TableRow
                      key={targetKey(row.target)}
                      // 有货的行整行铺一层有货色。这一刻是这个程序存在的全部理由，
                      // 不该只靠一枚小徽标。
                      className={cn(
                        "odd:bg-muted/30",
                        row.availability.kind === "in_stock" &&
                          "bg-in-stock/10 odd:bg-in-stock/10 hover:bg-in-stock/15",
                      )}
                    >
                      <TableCell className="pl-4">
                        <StatusBadge availability={row.availability} />
                      </TableCell>
                      <TableCell className="font-medium">{row.target.storeTitle}</TableCell>
                      <TableCell className="text-muted-foreground">
                        {productLabel(row.target.productName)}
                        {row.availability.kind === "unknown" && row.availability.reason === "pickup_not_open" && (
                          <div className="mt-1 text-xs">Apple：{row.availability.message}</div>
                        )}
                      </TableCell>
                      <TableCell>
                        <Switch
                          aria-label={t(`重要警告：${row.target.storeTitle} ${productLabel(row.target.productName)}`)}
                          checked={row.target.criticalAlert ?? false}
                          onCheckedChange={(enabled) => void setTargets(targets.map((t) =>
                            targetKey(t) === targetKey(row.target) ? { ...t, criticalAlert: enabled } : t
                          ), targets)}
                        />
                      </TableCell>
                      <TableCell className="text-muted-foreground tabular-nums">
                        {formatTime(row.lastCheckedMs)}
                      </TableCell>
                      <TableCell>
                        <Button
                          variant="ghost"
                          size="icon"
                          aria-label={t("删除这条监控")}
                          onClick={() => void onRemove(row.target)}
                        >
                          <Trash2 />
                        </Button>
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </ScrollArea>
        </section>

        <section className="bg-card h-28 shrink-0 overflow-hidden rounded-xl border shadow-xs">
          <ScrollArea className="h-full">
            <div className="p-3 font-mono text-xs leading-5 select-text">
              {ui.logs.length === 0 ? (
                <p className="text-muted-foreground">{t("日志会显示在这里。")}</p>
              ) : (
                ui.logs.map((line, i) => <LogLine key={i} line={line} />)
              )}
            </div>
          </ScrollArea>
        </section>
      </div>
    </TooltipProvider>
  );
}
