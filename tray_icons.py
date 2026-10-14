"""
系统托盘图标渲染模块

采用 4x 超采样矢量空间 (128x128 SSAA) 进行高精度几何与文字绘制，
并通过 Lanczos 高质量平滑降采样输出无锯齿、无模糊的高清托盘位图。
针对 Windows 11/10 现代任务栏提供胶囊电池、竖向能量柱、极简纯数字与仪表圆环 4 种样式。
具备极致画布充满度设计，杜绝四周多余留白导致的缩小失真，文字硕大锐利、一目了然。
"""

import ctypes
from ctypes import wintypes
from functools import lru_cache
import os
from pathlib import Path
from types import MappingProxyType

from PIL import Image, ImageDraw, ImageFont
from config import (
    TRAY_ICON_STYLE_CAPSULE,
    TRAY_ICON_STYLE_VERTICAL,
    TRAY_ICON_STYLE_NUMBER,
    TRAY_ICON_STYLE_CIRCLE,
)

# 超采样基础画布尺寸 (128x128 矢量画布，降采样至目标托盘尺寸)
SUPERSAMPLE_CANVAS_SIZE = 128

# 现代 Windows 任务栏托盘图标推荐基础物理网格 (100% 缩放下为 32px，避免 16px 放大拉伸模糊)
BASE_ICON_SIZE = 32
BASE_SCALE_PERCENT = 100

# 优先选用的 Windows 现代粗体无衬线字体，路径直接基于 Windows 字体目录
FONT_CANDIDATES = ('segoeuib.ttf', 'SegoeUI-Bold.ttf', 'arialbd.ttf')

# 状态配色方案 (RGB 元组)：涵盖充电、电量充足、电量预警、严重低电与设备离线
COLORS = MappingProxyType({
    'charging': (6, 182, 212),     # 充电中：青蓝科技色
    'good': (16, 185, 129),        # 电量良好 (>=50%)：翡翠绿
    'warning': (245, 158, 11),     # 电量预警 (20%~49%)：琥珀橙
    'low': (239, 68, 68),          # 严重低电 (<20%)：警示红
    'unknown': (148, 163, 184),    # 状态未知或设备离线：哑光灰
})


def taskbar_icon_size() -> int:
    """
    按当前主任务栏所在显示器的 DPI 缩放计算托盘图标的物理像素尺寸。

    基于 32px 物理标准基准，通过 Windows shcore API 读取缩放比，
    100% 缩放交付 32px，125% 交付 40px，150% 交付 48px，200% 交付 64px。
    """
    try:
        user = ctypes.WinDLL('user32', use_last_error=True)
        user.MonitorFromPoint.argtypes = [wintypes.POINT, wintypes.DWORD]
        user.MonitorFromPoint.restype = wintypes.HMONITOR
        monitor = user.MonitorFromPoint(wintypes.POINT(0, 0), 2)

        shcore = ctypes.WinDLL('shcore', use_last_error=True)
        shcore.GetScaleFactorForMonitor.argtypes = [wintypes.HMONITOR, ctypes.POINTER(ctypes.c_int)]
        shcore.GetScaleFactorForMonitor.restype = ctypes.c_long
        scale = ctypes.c_int()
        result = shcore.GetScaleFactorForMonitor(monitor, ctypes.byref(scale))
        if result == 0 and scale.value > 0:
            return max(BASE_ICON_SIZE, round(BASE_ICON_SIZE * scale.value / BASE_SCALE_PERCENT))
    except Exception:
        pass
    return BASE_ICON_SIZE


@lru_cache(maxsize=32)
def get_font(size: int) -> ImageFont.FreeTypeFont:
    """
    缓存加载指定字号的 Windows 系统字体。

    Args:
        size: 在 128px 超采样画布中的字体像素尺寸。
    """
    fonts_dir = Path(os.environ.get('WINDIR', 'C:/Windows')) / 'Fonts'
    for candidate in FONT_CANDIDATES:
        font_path = fonts_dir / candidate
        if font_path.exists():
            try:
                return ImageFont.truetype(str(font_path), size)
            except Exception:
                continue
    return ImageFont.load_default()


def battery_color(percentage: int, charging: bool) -> tuple[int, int, int]:
    """
    根据电量百分比与充电状态获取对应的状态标识色。

    Args:
        percentage: 电量百分比 (0-100)，负数代表离线/未知。
        charging: 是否处于充电状态。
    """
    if percentage < 0:
        return COLORS['unknown']
    if charging:
        return COLORS['charging']
    if percentage >= 50:
        return COLORS['good']
    if percentage >= 20:
        return COLORS['warning']
    return COLORS['low']


def draw_capsule(draw: ImageDraw.ImageDraw, options: dict):
    """
    绘制横向胶囊电池样式：最大化利用垂直与水平高度，杜绝多余留白。

    内部文字硕大清晰（字号 70 / 三位数 56），包裹于安全区内，绝不溢出。
    """
    bx0, by0, bx1, by1 = 4, 14, 112, 114
    radius = 18
    shell_color = options['shell_color']
    track_color = options['track_color']
    color = options['color']
    percentage = options['percentage']
    text = options['text']

    # 电池外壳与半透明底槽
    draw.rounded_rectangle([bx0, by0, bx1, by1], radius=radius, fill=track_color, outline=shell_color, width=5)
    # 右侧正极触点
    draw.rounded_rectangle([bx1 - 2, 42, 126, 86], radius=7, fill=shell_color)

    # 有效电量充盈槽
    if percentage > 0:
        fill_pct = min(100, max(0, percentage)) / 100.0
        inner_w = (bx1 - 6) - (bx0 + 6)
        cur_w = max(20, int(inner_w * fill_pct))
        draw.rounded_rectangle([bx0 + 6, by0 + 6, bx0 + 6 + cur_w, by1 - 6], radius=13, fill=color)

    # 硕大粗体居中文字
    font_size = 56 if len(text) >= 3 else 70
    draw.text(
        ((bx0 + bx1) // 2, (by0 + by1) // 2),
        text,
        font=get_font(font_size),
        anchor='mm',
        fill=(255, 255, 255),
        stroke_width=3,
        stroke_fill=(0, 0, 0, 180),
    )


def draw_vertical(draw: ImageDraw.ImageDraw, options: dict):
    """
    绘制竖向微型能量柱样式：最大化利用宽度与高度，饱满撑满任务栏槽位。

    数字硕大居中（字号 70 / 三位数 56），彻底告别文字爆出边框。
    """
    vx0, vy0, vx1, vy1 = 14, 18, 114, 126
    radius = 18
    shell_color = options['shell_color']
    track_color = options['track_color']
    color = options['color']
    percentage = options['percentage']
    text = options['text']

    # 顶部正极触点
    draw.rounded_rectangle([42, 2, 86, 20], radius=7, fill=shell_color)
    # 电池柱外壳与底槽
    draw.rounded_rectangle([vx0, vy0, vx1, vy1], radius=radius, fill=track_color, outline=shell_color, width=5)

    # 自底向上的能量充盈槽
    if percentage > 0:
        fill_pct = min(100, max(0, percentage)) / 100.0
        inner_h = (vy1 - 6) - (vy0 + 6)
        cur_h = max(20, int(inner_h * fill_pct))
        draw.rounded_rectangle([vx0 + 6, vy1 - 6 - cur_h, vx1 - 6, vy1 - 6], radius=13, fill=color)

    # 硕大粗体居中文字
    font_size = 56 if len(text) >= 3 else 70
    draw.text(
        ((vx0 + vx1) // 2, (vy0 + vy1) // 2 + 2),
        text,
        font=get_font(font_size),
        anchor='mm',
        fill=(255, 255, 255),
        stroke_width=3,
        stroke_fill=(0, 0, 0, 180),
    )


def draw_number(draw: ImageDraw.ImageDraw, options: dict):
    """
    绘制极简纯数字样式：超大饱满实心大字 (字号 102 / 三位数 80)，去除产生雾化感的小描边。
    """
    text = options['text']
    color = options['color']

    # 针对 100 与两位数进行视觉平衡字号微调
    font_size = 80 if len(text) >= 3 else 102
    draw.text(
        (64, 64),
        text,
        font=get_font(font_size),
        anchor='mm',
        fill=color,
    )


def draw_circle(draw: ImageDraw.ImageDraw, options: dict):
    """
    绘制仪表圆环样式：饱满动态进度圆弧 + 紧凑贴合内衬 + 硕大居中数字。

    圆环线宽采用 15px（占画布约 11.7%），外径半径 60px 充分利用画布边缘；
    内部空间宽裕，数字字号大幅提升至 70px（三位数 54px），兼顾外圈清晰度与核心数字辨识度。
    """
    percentage = options['percentage']
    text = options['text']
    color = options['color']
    dark = options['dark']

    center = 64
    radius = 60
    line_width = 15
    box = [center - radius, center - radius, center + radius, center + radius]

    # 智能自适应表盘内衬与高对比度文字：根据任务栏深浅色自动提供衬底防穿透
    if dark:
        inner_disc = (15, 20, 28, 160)
        track_arc_color = (255, 255, 255, 60)
        text_color = (255, 255, 255)
        stroke_color = (0, 0, 0, 180)
    else:
        inner_disc = (255, 255, 255, 200)
        track_arc_color = (0, 0, 0, 45)
        text_color = (15, 23, 42)
        stroke_color = (255, 255, 255, 200)

    # 1. 绘制内部表盘底座：半径紧凑贴合圆环内径 (radius - line_width = 45px)，提供充足字符展示区
    inner_r = radius - line_width
    draw.ellipse([center - inner_r, center - inner_r, center + inner_r, center + inner_r], fill=inner_disc)

    # 2. 全圆底槽轨道：清晰展现完整表盘闭环轮廓
    draw.arc(box, start=0, end=360, fill=track_arc_color, width=line_width)

    # 3. 动态电量进度弧线 (从正上方 12 点钟位置顺时针绘制)
    if percentage > 0:
        sweep = 360 * (min(100, percentage) / 100.0)
        draw.arc(box, start=-90, end=-90 + sweep, fill=color, width=line_width)

    # 4. 核心区域居中电量数字：两位数采用 70px 饱满大字，三位数（100）采用 54px
    font_size = 54 if len(text) >= 3 else 70
    draw.text(
        (center, center),
        text,
        font=get_font(font_size),
        anchor='mm',
        fill=text_color,
        stroke_width=2,
        stroke_fill=stroke_color,
    )


def draw_charging_bolt(draw: ImageDraw.ImageDraw):
    """
    在右上角专用微标角标区绘制发光金色闪电，杜绝遮挡切断主体数字。
    """
    bolt_points = [(108, 4), (90, 26), (102, 26), (94, 48), (120, 20), (106, 20)]
    draw.polygon(bolt_points, fill=(250, 204, 21), outline=(0, 0, 0, 180))


def create_battery_icon(
    percentage: int = -1,
    charging: bool = False,
    *,
    style: str = TRAY_ICON_STYLE_CAPSULE,
    is_dark_taskbar: bool = True,
    size: int | None = None,
) -> Image.Image:
    """
    生成当前任务栏目标尺寸的高质量 RGBA 托盘图标。

    Args:
        percentage: 电量百分比 (-1 到 100)，负数代表未知/离线。
        charging: 是否正在充电。
        style: 托盘图标样式 (capsule / vertical / number / circle)。
        is_dark_taskbar: Windows 任务栏是否为深色模式。
        size: 目标物理像素尺寸，留空则自动按当前显示器 DPI 计算。

    Returns:
        包含完整抗锯齿的高质量 PIL RGBA 图像对象。
    """
    target_size = taskbar_icon_size() if size is None else size
    if percentage < -1 or percentage > 100 or target_size <= 0:
        raise ValueError('非法电量或托盘图标尺寸')

    # 在 128x128 矢量画布上超采样绘制
    canvas = Image.new('RGBA', (SUPERSAMPLE_CANVAS_SIZE, SUPERSAMPLE_CANVAS_SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(canvas)

    # 任务栏深浅色环境配色自适应
    shell_color = (255, 255, 255, 230) if is_dark_taskbar else (30, 41, 59, 230)
    track_color = (255, 255, 255, 45) if is_dark_taskbar else (0, 0, 0, 45)
    text_color = (255, 255, 255) if is_dark_taskbar else (15, 23, 42)

    options = {
        'percentage': percentage,
        'text': '--' if percentage < 0 else str(percentage),
        'color': battery_color(percentage, charging),
        'dark': is_dark_taskbar,
        'shell_color': shell_color,
        'track_color': track_color,
        'text_color': text_color,
    }

    if style == TRAY_ICON_STYLE_CAPSULE:
        draw_capsule(draw, options)
    elif style == TRAY_ICON_STYLE_VERTICAL:
        draw_vertical(draw, options)
    elif style == TRAY_ICON_STYLE_NUMBER:
        draw_number(draw, options)
    elif style == TRAY_ICON_STYLE_CIRCLE:
        draw_circle(draw, options)
    else:
        raise ValueError(f'非法托盘图标样式：{style}')

    # 充电时在右上角微标位置绘制清晰闪电
    if charging:
        draw_charging_bolt(draw)

    # 高质量 Lanczos 降采样至目标尺寸，获得极致平滑边缘
    if target_size == SUPERSAMPLE_CANVAS_SIZE:
        return canvas
    return canvas.resize((target_size, target_size), resample=Image.Resampling.LANCZOS)
