import { api, el } from './dom.js';

// 低电量阈值沿用旧 GUI 的四档业务规则。
const NOTIFY_LEVELS = Object.freeze([0, 10, 20, 30]);
// 分段选择器 ID 对应配置键，修改时只写入这一个偏好。
const CHOICES = Object.freeze({ languageControl: 'ui_language', trayIconStyleControl: 'tray_icon_style',
    themeModeControl: 'theme_mode' });
// 独立开关：自动刷新只属于当前 GUI 会话。
const SWITCHES = Object.freeze({ autoUpdateSwitch: 'auto_update', autostartSwitch: 'autostart',
    autoRefreshSwitch: 'auto_refresh' });

export class Settings {
    constructor(options) {
        this.t = options.t;
        this.toast = options.toast;
        this.acceptState = options.acceptState;
        this.config = null;
        this.bindControls();
    }

    // 每个控件只保存其对应配置，写入失败后恢复实际配置并明确提示。
    bindControls() {
        Object.entries(CHOICES).forEach(([id, key]) => {
            el(id).addEventListener('click', event => {
                const button = event.target.closest('.seg-btn');
                if (button) this.save(key, button.dataset.value);
            });
        });
        Object.entries(SWITCHES).forEach(([id, key]) => {
            el(id).addEventListener('change', () => this.save(key, el(id).checked));
        });
        el('trayPrioritySelect').addEventListener('change', () => {
            this.save('tray_icon_priority', el('trayPrioritySelect').value);
        });
        el('btnLowBatteryDec').addEventListener('click', () => this.changeThreshold(-1));
        el('btnLowBatteryInc').addEventListener('click', () => this.changeThreshold(1));
        el('btnGlassDec').addEventListener('click', () => this.changeGlass(-5));
        el('btnGlassInc').addEventListener('click', () => this.changeGlass(5));
    }

    async changeGlass(delta) {
        if (this.glassSaving) return;
        const current = this.config.glass_transparency ?? 25;
        const value = Math.max(0, Math.min(100, current + delta));
        if (value === current) return;
        this.glassSaving = true;
        el('btnGlassDec').disabled = el('btnGlassInc').disabled = true;
        try {
            await this.save('glass_transparency', value);
        } finally {
            this.glassSaving = false;
            this.applyGlass(this.config.glass_transparency ?? 25);
        }
    }

    // 保留原生 Acrylic、背景 RGB/渐变、布局和文字，仅减薄背景色层。
    applyGlass(value) {
        const tint = 1 - value / 100;
        document.body.style.setProperty('--glass-dark-start', String(0.84 * tint));
        document.body.style.setProperty('--glass-dark-end', String(0.88 * tint));
        document.body.style.setProperty('--glass-light-start', String(0.82 * tint));
        document.body.style.setProperty('--glass-light-end', String(0.75 * tint));
        el('glassTransparencyValue').textContent = `${value}%`;
        el('btnGlassDec').disabled = this.glassSaving || value <= 0;
        el('btnGlassInc').disabled = this.glassSaving || value >= 100;
    }

    // 未列出的历史阈值从其实际大小向相邻合法档位切换，不能伪造保存成功。
    changeThreshold(direction) {
        const current = this.config.low_battery_notify;
        const values = direction < 0 ? [...NOTIFY_LEVELS].reverse() : NOTIFY_LEVELS;
        const next = values.find(value => direction < 0 ? value < current : value > current);
        if (next !== undefined) this.save('low_battery_notify', next);
    }

    // Python 返回成功后才应用偏好；异常由提示展示，不能仅改变控件外观。
    async save(key, value) {
        try {
            this.acceptState(await api('set_config_value', key, value));
        } catch (error) {
            this.apply(this.config);
            this.toast(this.t('toast.saving_failed', { msg: error.message || error }));
        }
    }

    // 完整配置来自 Python，自动语言不用浏览器语言猜测。
    apply(config) {
        this.config = { ...config };
        Object.entries(CHOICES).forEach(([id, key]) => {
            Array.from(el(id).children).forEach(button => {
                button.classList.toggle('active', button.dataset.value === config[key]);
            });
        });
        Object.entries(SWITCHES).forEach(([id, key]) => { el(id).checked = config[key]; });
        el('trayPrioritySelect').value = config.tray_icon_priority;
        el('lowBatteryValue').textContent = config.low_battery_notify === 0
            ? this.t('settings.off') : `${config.low_battery_notify}%`;
        document.body.classList.remove('theme-auto', 'theme-light', 'theme-dark');
        document.body.classList.add(`theme-${config.theme_mode}`);
        this.applyGlass(config.glass_transparency ?? 25);
    }
}
