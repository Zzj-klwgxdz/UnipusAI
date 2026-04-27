# U校园AI版刷课脚本
### 大学英语的刷课可谓既浪费时间又没有太多意义，我曾经在网上查过很多关于U校园AI版的刷课脚本，但是都无法使用。于是我决定既然没有那就自己创造。
### 作者Bilibili主页[点击查看](https://space.bilibili.com/556022848)

### [视频教程](https://pan.baidu.com/s/13MTMXX0US9DUdn9_ivDfaQ) 提取码: 69eq


## 主要功能


- **功能1:** 全自动程序，解放双手
- **功能2:** 接入KIMI API，实现AI答题



## 使用技术


- HTML, JavaScript
- Python，Selenium

## 环境依赖
- 本项目需要Python环境
- 需要安装Microsoft Edge浏览器和Microsoft Edge WebDriver驱动程序[点击下载](https://developer.microsoft.com/zh-cn/microsoft-edge/tools/webdriver)
- 2.0及以后版本需要额外安装FFmpeg来实现网页音频和视频的解析,详见v2.0更新
## 使用方法
### 对于想要了解代码的用户
1. 在PyCharm或其他IDE中克隆此仓库
2. 运行``` pip install -r requirements.txt ```安装依赖
3. 修改config.json
4. 运行Unipus_vx.x.py
### 对于只想要体验程序功能的用户
- 只需要下载右侧release里的dist.zip，解压后编辑config.json，运行exe即可
- 仍然需要安装Microsoft Edge浏览器和Microsoft Edge WebDriver驱动程序[点击下载](https://developer.microsoft.com/zh-cn/microsoft-edge/tools/webdriver)
## 注意：启动后严禁一切操作，否则可能导致程序异常
## 在config.json里编辑配置
1. 把"Your username"替换为你的账号，把"Your password"替换为你的密码
2. 把“Your api"替换为你在[KIMI开放平台](https://platform.moonshot.cn/docs/guide/start-using-kimi-api)申请的API KEY(无需修改base_url和model)<p></p>或者任意支持openai接口的大模型api并修改相应的base_url和model

3. 两种学习策略,"learn_all"为学习所有课程，“learn_all_compulsory_course”为学习必修课
4. full_token需要改为自己的token，详见《关于U校园ai版的防作弊机制》
5. whisper_api用于语音识别的在线模式，一般情况下不用管，值为null时使用本地模型，初次启动程序时会自动下载本地模型
## 关于U校园ai版的防作弊机制
- 我已破解
- 手动在浏览器登陆账号，然后打开开发者窗口在控制台输入localStorage.getItem('__token')
把获取的token粘贴到config.json中的token_full中（注意格式一致）
- 此token会不定期更新，如果发现登陆进去是白屏，那么需要更新token

## 补充说明：由于程序是根据读写教程编写的，所以在处理视听说教程时可能无法正常使用，有待后续完善
## v1.1更新
新增config编辑工具，位于
### /net10.0-windows/ConfigEditor.exe
## v2.0更新
### ![red](https://img.shields.io/badge/注意-red)2.0及以后版本需要额外安装FFmpeg来实现网页音频和视频的解析
### 安装步骤
1. 访问[此网站](https://github.com/BtbN/FFmpeg-Builds/releases)，下载ffmpeg-master-latest-win64-gpl.zip
2. 将文件解压到任意目录，推荐C盘根目录
3. 添加系统环境变量：<p></p>
&emsp;首先按Win+R，输入sysdm.cpl,确定。<p></p>&emsp;然后点高级 ->环境变量 -> 在系统变量中找到Path,编辑 -> 新建 -> 输入你的解压的路径+\bin ![yellow](https://img.shields.io/badge/例如-yellow) C:\ffmpeg\bin  -> 一直点击确定即可

### 新增了以下题型的支持，通过接入语音识别功能实现
![image](QQ20260307-183216.png)
### 修复了部分情况下选择题ai调用失败的问题
### 修复了有时阅读题答案用中文回答的问题
## v2.1更新
- 新增了环境检测
- 增加了目标课程的模糊匹配
- 修复了其他浏览器修改配置导致此程序异常
## v2.2更新
- 将选择课程改为手动选择，请在进入目录前关注控制台
## 📜 许可证

本项目在[MIT License](LICENSE)下发布。