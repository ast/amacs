#include <string>
#include <vector>

namespace shapes {

class Shape {
public:
  virtual ~Shape() = default;
  virtual double area() const = 0;

private:
  std::string name_;
};

template <typename T>
T largest(const std::vector<T> &items) {
  T best = items.front();
  for (const auto &item : items) {
    if (item > best) {
      best = item;
    }
  }
  return best;
}

} // namespace shapes

int main() {
  auto values = std::vector<int>{1, 2, 3};
  return shapes::largest(values) == 3 ? 0 : 1;
}
