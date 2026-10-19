<p align="center"><a href="README.md">English</a> · <a href="README.ko.md">한국어</a> · <a href="README.zh.md">中文</a></p>

<p align="center"><img src="assets/icon-256.png" width="96" alt="Flick"></p>

<p align="center"><strong><a href="https://github.com/philaxis/flick/releases/latest/download/Flick.exe">⬇ Windows용 Flick 받기</a></strong></p>

<p align="center">버튼을 누른 채 마우스를 밀면 옆 데스크톱입니다.</p>

<p align="center"><a href="https://philaxis.github.io/flick/">Flick 웹사이트에서 동작 보기</a></p>

---

**데스크톱이 격자가 됩니다.** 좌우는 한 작업의 화면들, 위아래는 다른 작업입니다.

**누른 채 밀기.** 트리거를 누르고 마우스를 밀면 그 방향으로 한 칸 넘어갑니다. 커서는 제자리에 있습니다.

**눌렀다 떼면 전체 보기.** 지금 칸의 창들과 작은 지도가 뜹니다. 창을 지도의 칸으로 끌면 옮겨집니다.

**버튼은 마음대로.** 처음에는 마우스 "앞으로" 버튼입니다. 트레이 아이콘 우클릭 → *Settings…* → *Change*를 누른 뒤 원하는 버튼이나 키 여러 개를 누르면 그걸로 바뀝니다.

**작고 무료.** 1MB도 안 되는 실행 파일 하나. 가입도 설치 화면도 없습니다. 오픈 소스(MIT).

**세 손가락 터치패드.** Windows의 세 손가락 밀기와 탭을 ‘아무 것도 안 함’으로 설정한 뒤 Flick 설정에서 켜세요. 탭하면 보드가 열리고, 누른 채 약 1cm 움직이면 데스크톱 하나를 이동합니다. 기본으로는 터치 스크롤처럼 화면을 밀듯 손가락과 반대 방향으로 이동하고, 마우스는 손을 따라갑니다.

**마우스와 터치패드를 따로.** 설정에 *Mouse*와 *Touchpad* 탭이 있습니다. 탭마다 좌우(같은 행 안)와 위아래(행 바꾸기)를 따로 정합니다: 데스크톱 하나에 움직일 거리(마우스는 픽셀, 터치패드는 mm)와, 칸마다 걸리는 세기(*Notch*, Off면 손을 고르게 따라감). *Reverse direction*은 그 장치의 방향을 뒤집습니다. config.toml에서는 `[mouse]`, `[touchpad]` 아래에 같은 설정이 있고, 이전 버전의 config.toml은 마우스 설정으로 읽힙니다.

**화면이 손을 따라옵니다.** *Follows the hand*를 켜 두면(기본, 방향마다 따로) Mac처럼 데스크톱이 마우스나 손가락을 따라 미끄러지며 다음 데스크톱이 들어오고, 손을 뗄 때 한 번만 전환됩니다: 가장 가까운 데스크톱으로, 빠르게 튕기면 그만큼 더 멀리. 모든 행에 고정한 창은 제자리에, 이 행에 고정한 창은 좌우로 움직일 때 제자리에 있습니다. 끄면 손이 닿는 데스크톱마다 바로 전환합니다.

**화면이 곧 지도입니다.** 움직이는 동안 화면이 뒤로 물러나 상하좌우와 대각선 이웃 데스크톱이 한 칸씩 보이고, 가운데 데스크톱은 조금 크고 밝게 떠 있으며, 화면 위에 셀프 이름이 뜹니다. 한 행 안에서 좌우로 움직이면 그 행만 움직이고 위아래 행은 제자리에 있습니다. 움직일 때 뜨던 작은 미니맵은 기본으로 꺼져 있습니다. 설정의 *Minimap while moving*(또는 config.toml의 `minimap = true`)으로 다시 켤 수 있습니다.

## 시작하기

1. 위 버튼으로 받아 실행합니다. 스스로 설치하고 윈도우와 함께 시작합니다.
2. 트리거를 누른 채 마우스를 옆으로 밉니다.
3. 트리거를 눌렀다 떼고, 지도의 **+**로 데스크톱이나 새 행을 추가합니다.

## 알아 둘 것

- 윈도우 11 23H2(빌드 22631.3085 이상), 24H2(빌드 26100.2605 이상), 25H2에서 동작합니다. 그 밖의 버전에서는 안내만 띄우고 실행하지 않습니다.
- 제거: 설정 → 앱 → 설치된 앱 → Flick.

## 빌드

윈도우에서 `cargo build --release`, WSL에서는 `scripts/build-wsl.sh build --release`.

[MIT](LICENSE). `vendor/winvd`와 `vendor/winvd-24h2`는 [VirtualDesktopAccessor](https://github.com/Ciantic/VirtualDesktopAccessor)(MIT)에서 가져왔습니다.
