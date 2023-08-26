# documents

## concept

Discord でスクショする. LINE みたいなやつ.

### ざっくりアーキテクチャ

- 永続化: なし
- 状態: あり
<!-- sep -->
- requirements:
  - cpu: high (for encodings)
  - gpu: no
  - mem: low
<!-- sep -->
- かなり小さいアプリケーションなので, 抽象化はあんまり要らない
  - でも保守性と testability のために随時分離は行う
  - 流石に carch は要らん
<!-- sep -->
