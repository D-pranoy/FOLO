from hypothesis import settings

# The default for every run; --hypothesis-profile=thorough tries far more inputs
settings.register_profile('default', max_examples=400, deadline=None)
settings.register_profile('thorough', max_examples=5000, deadline=None)
settings.load_profile('default')
