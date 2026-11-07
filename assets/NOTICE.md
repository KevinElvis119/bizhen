# 第三方资源与许可

本目录（assets/）下的字体资源版权归各自作者所有，随笔阵一起分发时遵循下列条款。
笔阵本身的代码以 GPL-3.0-or-later 发布（见仓库根目录 LICENSE）。

## assets/fonts/hershey/*.jhf

Hershey 单线字体，原始字形由 Dr. A. V. Hershey 在美国国家标准局工作期间创作，
后续由 James Hurt（Cognition, Inc.）整理为 `.jhf` 格式并公开分发。

* 来源：<https://github.com/kamalmostafa/hershey-fonts>
* 详见同目录的 `ACKNOWLEDGEMENT.txt`
* 该字体数据集长期被视为公有领域（public domain）资源

## assets/fonts/stroke-json/STRK-Kaiti.json

中文单线（Hershey 风格）楷体字形，由 chinese-hershey-font 项目从公开字形数据转换而来。

* 来源：<https://github.com/LingDong-/chinese-hershey-font>
* 许可：MIT

## 关于 .gfont

笔阵**不包含**任何 kvenjoy `.gfont` 格式的解析代码或字体样本。
如需使用 `.gfont` 字体，请用独立的 `gfont-tool` 项目先转换为 `stroke-json`：

<https://github.com/ZhengXieGang/WriterStudio> 之外，转换工具见 gfont-tool 仓库说明。
