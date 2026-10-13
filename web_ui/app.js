import { api, createToast, el } from './dom.js';
import { createTranslator } from './locale.js';
import { createDeviceRenderer } from './device_view.js';
import { DeviceDialogs } from './device_dialogs.js';
import { Settings } from './settings.js';
import { UpdateUi } from './update_ui.js';

class BatteryApp {
    constructor(services) {
        this.translator = services.translator;
        this.toast = services.toast;
        this.dialogs = services.dialogs;
        this.settings = services.settings;
        this.updates = services.updates;
        this.renderDevices = services.renderDevices;
        this.configSignature = null;
        this.settingsOpen = false;
        this.bindActions();
        this.exposeBridge();
    }

    // Python 的状态、模式和更新事件分别交给对应责任模块。
    exposeBridge() {
        window.__onBatteryStateUpdated = state => this.acceptState(state);
        window.__onUpdateStatus = event => this.updates.update(event);
        window.__setViewMode = mode => this.setViewMode(mode);
    }

    // 接收实际快照后统一刷新语言、设置、卡片和在途绑定状态。
    acceptState(state) {
        const signature = JSON.stringify(state.config);
        if (signature !== this.configSignature) {
            this.translator.apply(state.config);
            this.settings.apply(state.config);
            this.configSignature = signature;
        }
        el('appVersion').textContent = `v${state.version.replace(/^v/, '')}`;
        this.renderDevices(state, this.translator.t);
        this.dialogs.update(state);
    }

    // 所有入口保留真实执行反馈，按钮在该次操作完成前防止重复提交。
    bindActions() {
        el('btnRefresh').addEventListener('click', () => this.deviceAction('refresh_battery', 'refresh'));
        ['btnScanDevices', 'btnEmptyScan'].forEach(id => {
            el(id).addEventListener('click', () => this.deviceAction('scan_devices', 'scan'));
        });
        el('btnAddKeyboard').addEventListener('click', () => this.dialogs.add('keyboard'));
        ['btnAddBluetooth', 'btnEmptyAddBt'].forEach(id => {
            el(id).addEventListener('click', () => this.dialogs.add('bluetooth'));
        });
        el('deviceList').addEventListener('click', event => {
            const button = event.target.closest('[data-action="remove"]');
            if (button) this.dialogs.remove(button.dataset.type, {
                id: button.dataset.id, name: button.dataset.name,
            });
        });
        el('btnToggleSettings').addEventListener('click', () => this.toggleSettings());
        el('btnClose').addEventListener('click', () => this.close());
        document.querySelector('.flyout-header').addEventListener('mousedown', event => {
            if (event.button === 0 && !event.target.closest('button')) {
                api('drag_window').catch(error => this.operationError(error));
            }
        });
        document.addEventListener('keydown', event => {
            if (event.key === 'Escape') this.close();
        });
    }

    // 只有硬件回执成功才提示完成；失败时恢复按钮并显示原因。
    async deviceAction(method, kind) {
        const controls = kind === 'refresh' ? [el('btnRefresh')]
            : [el('btnScanDevices'), el('btnEmptyScan')];
        controls.forEach(button => { button.disabled = true; });
        if (kind === 'refresh') el('btnRefresh').classList.add('rotating');
        this.toast(this.translator.t(`toast.${kind}_started`));
        try {
            this.acceptState(await api(method));
            this.toast(this.translator.t(`toast.${kind}_completed`));
        } catch (error) {
            this.operationError(error);
        } finally {
            controls.forEach(button => { button.disabled = false; });
            if (kind === 'refresh') el('btnRefresh').classList.remove('rotating');
        }
    }

    // 齿轮只切换当前窗口的内容，原生窗体模式由托盘消息控制。
    toggleSettings() {
        this.settingsOpen = !this.settingsOpen;
        this.applyView();
    }

    // 设置大窗口允许原生拖动，浮窗模式继续保持任务栏吸附。
    setViewMode(mode) {
        this.settingsOpen = mode === 'settings';
        document.body.classList.toggle('window-mode', this.settingsOpen);
        this.applyView();
    }

    // 设置与设备面板互斥，所有设备管理入口均可从齿轮返回。
    applyView() {
        el('deviceView').classList.toggle('active', !this.settingsOpen);
        el('settingsView').classList.toggle('active', this.settingsOpen);
        el('btnToggleSettings').style.color = this.settingsOpen ? 'var(--accent-green)' : '';
    }

    // Python 负责下载取消与进程收尾，不在前端立即宣布窗口已销毁。
    async close() {
        try {
            await api('hide_window');
        } catch (error) {
            this.operationError(error);
        }
    }

    // 操作异常统一展示真实消息，不改写成空结果或成功状态。
    operationError(error) {
        this.toast(this.translator.t('toast.op_failed', { msg: error.message || error }));
    }

    // 先取得状态并完成渲染，最后通知 Python 展示首屏。
    async initialize() {
        this.acceptState(await api('get_state'));
        await api('on_frontend_ready');
    }
}

// 桥接就绪后组合并渲染首屏；组件创建和渲染异常均交给原生错误提示。
async function initializeApp() {
    try {
        const translator = createTranslator();
        const toast = createToast();
        const acceptState = state => app.acceptState(state);
        const dialogs = new DeviceDialogs({ t: translator.t, toast, acceptState });
        const settings = new Settings({ t: translator.t, toast, acceptState });
        const updates = new UpdateUi({ t: translator.t, toast });
        const app = new BatteryApp({ translator, toast, dialogs, settings, updates,
            renderDevices: createDeviceRenderer() });
        await app.initialize();
    } catch (error) {
        console.error('GUI initialization failed', error);
        await api('on_frontend_error', String(error.message || error));
    }
}

if (window.pywebview?.api) initializeApp();
else window.addEventListener('pywebviewready', initializeApp, { once: true });
